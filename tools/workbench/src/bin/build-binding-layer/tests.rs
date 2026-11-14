use super::*;

const GENERATIONS_PER_BUILD: usize = binding_layer::GENERATIONS.len();

const INSTALL: &str = "rustup toolchain install";

const TARGET_ADD: &str = "rustup target add";

const CONTAINER_ID: &str = "container-id";

fn host_roots() -> Result<Roots, Vec<String>> {
    Roots::on_host(
        path::PathBuf::from(HOST_ROOT),
        path::PathBuf::from(OUTSIDE_OUT),
    )
}

fn every_plan() -> Result<[Vec<Step>; 3], Vec<String>> {
    let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
    Ok([
        android_plan(&roots()?, &abis),
        ios_plan(&host_roots()?, IosPhase::Full),
        kotlin_plan(&host_roots()?),
    ])
}

fn position_of(commands: &[String], leading: &str) -> Option<usize> {
    commands
        .iter()
        .position(|command| command.starts_with(leading))
}

#[test]
fn every_plan_clears_its_output_then_installs_the_pin_before_its_first_cargo_step(
) -> Result<(), Vec<String>> {
    for plan in every_plan()? {
        assert!(matches!(plan.first(), Some(Step::FreshDir(_))));
        let commands = commands(&plan);
        let install = workbench::required(
            commands.iter().position(|command| command == INSTALL),
            "toolchain install step",
        )?;
        let cargo = workbench::required(position_of(&commands, "cargo "), "cargo step")?;
        assert!(install < cargo);
        assert!(commands.iter().all(|command| !command.contains("stable")));
    }
    Ok(())
}

#[test]
fn android_plan_adds_only_the_selected_targets_to_the_pinned_toolchain() -> Result<(), Vec<String>>
{
    let x86: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS
        .iter()
        .filter(|abi| abi.jni_dir == "x86")
        .collect();
    let commands = commands(&android_plan(&roots()?, &x86));
    let target_add = workbench::required(position_of(&commands, TARGET_ADD), "target add step")?;
    assert_eq!(
        commands[target_add],
        format!("{TARGET_ADD} {}", x86[FIRST_POSITION].triple)
    );
    Ok(())
}

#[test]
fn ios_plan_adds_the_device_and_simulator_targets_to_the_pinned_toolchain(
) -> Result<(), Vec<String>> {
    let commands = commands(&ios_plan(&host_roots()?, IosPhase::Full));
    let target_add = workbench::required(position_of(&commands, TARGET_ADD), "target add step")?;
    let targets: Vec<&str> = iter_ios_targets().map(|target| target.triple()).collect();
    assert_eq!(
        commands[target_add],
        format!("{TARGET_ADD} {}", targets.join(" "))
    );
    Ok(())
}

fn ios_static_library(lib_name: &str) -> String {
    binding_layer::library_file(
        binding_layer::LIBRARY_PREFIX,
        lib_name,
        binding_layer::STATIC_SUFFIX,
    )
}

#[test]
fn the_ios_plan_hides_every_symbol_but_the_uniffi_exports_of_both_crates() -> Result<(), Vec<String>>
{
    let commands = commands(&ios_plan(&host_roots()?, IosPhase::Full));
    let archive_file = ios_static_library(binding_layer::WALLET_LIB_NAME);
    let object_file = binding_layer::library_file(
        binding_layer::LIBRARY_PREFIX,
        binding_layer::WALLET_LIB_NAME,
        OBJECT_SUFFIX,
    );
    let exports = [
        binding_layer::WALLET_LIB_NAME,
        binding_layer::PROXY_LIB_NAME,
    ]
    .map(|lib_name| {
        format!("-exported_symbol _uniffi_{lib_name}_* -exported_symbol _ffi_{lib_name}_*")
    })
    .join(" ");
    for target in iter_ios_targets() {
        let slice_dir = format!("{}/{}", ios_slices(), target.triple());
        let archive = format!("{slice_dir}/{archive_file}");
        let hidden_archive = format!("{slice_dir}/{HIDDEN_DIR}/{archive_file}");
        let object = format!("{slice_dir}/{HIDDEN_DIR}/{object_file}");
        let link = format!(
            "ld -r -arch {} -platform_version {} {IOS_DEPLOYMENT_TARGET} {IOS_DEPLOYMENT_TARGET} -force_load {archive} {exports} -o {object}",
            target.arch(),
            target.platform()
        );
        let pack = format!("libtool -static -o {hidden_archive} {object}");
        let link_at = workbench::required(
            commands.iter().position(|command| command == &link),
            "the relocatable link",
        )?;
        let pack_at = workbench::required(
            commands.iter().position(|command| command == &pack),
            "the archive step",
        )?;
        let bindgen_at = commands
            .iter()
            .position(|command| command.contains(&format!("--library {archive} ")));
        assert!(link_at < pack_at);
        if target == IosTarget::Device {
            assert!(workbench::required(bindgen_at, "the bindgen step")? < link_at);
        } else {
            assert!(bindgen_at.is_none());
        }
        let mut readers = 0;
        for (at, command) in commands.iter().enumerate() {
            if command.contains(&archive) {
                assert!(
                    [Some(link_at), bindgen_at].contains(&Some(at)),
                    "{command} reads the raw {archive}"
                );
            }
            if command.contains(&hidden_archive) && at != pack_at {
                assert!(
                    pack_at < at,
                    "{command} reads {hidden_archive} before it exists"
                );
                readers += 1;
            }
        }
        assert!(readers > 0, "nothing reads {hidden_archive}");
    }
    Ok(())
}

#[test]
fn the_ios_plan_ships_one_rust_archive_so_the_app_links_one_std() -> Result<(), Vec<String>> {
    let commands = commands(&ios_plan(&host_roots()?, IosPhase::Full));
    let proxy_archive = ios_static_library(binding_layer::PROXY_LIB_NAME);
    assert!(commands
        .iter()
        .all(|command| !command.contains(&proxy_archive)));
    for target in iter_ios_targets() {
        let in_slice = format!("/{}/", target.triple());
        let links = commands
            .iter()
            .filter(|command| command.starts_with(&format!("{LD} ")))
            .filter(|command| command.contains(&in_slice))
            .count();
        assert_eq!(links, 1, "{} merges {links} archives", target.triple());
    }
    let xcframeworks = commands
        .iter()
        .filter(|command| command.starts_with("xcodebuild -create-xcframework"))
        .count();
    assert_eq!(xcframeworks, 1);
    Ok(())
}

#[test]
fn the_ios_plan_ends_with_the_swift_contract_test() -> Result<(), Vec<String>> {
    let plan = ios_plan(&host_roots()?, IosPhase::Full);
    let Some(Step::XcodeTest { workdir, scheme }) = plan.last() else {
        return Err(vec![
            "the iOS plan does not end with the Swift contract test".to_string(),
        ]);
    };
    assert_eq!(
        workdir,
        &format!("{HOST_ROOT}/{}", binding_layer::SWIFT_CONTRACT_TESTS_DIR)
    );
    assert_eq!(scheme, &binding_layer::swift_contract_tests_scheme());
    Ok(())
}

const IOS_WORKFLOW: &str = ".github/workflows/binding-layer-ios.yaml";
const XCODE_VERSION_KEY: &str = "XCODE_VERSION:";
const XCODE_VERSION_INPUT: &str = "xcode-version:";
const XCODE_VERSION_REFERENCE: &str = "${{ env.XCODE_VERSION }}";
const IOS_CACHE_KEY: &str = "key: ios-";
const IOS_RESTORE_KEY: &str = "ios-";
const MATRIX_TARGETS_KEY: &str = "target: [";
const CARGO_BUILD: &str = "cargo build";
const LIBRARY_FLAG: &str = "--library";
const ASSEMBLY_PROGRAMS: [&str; 4] = ["ld ", "libtool ", "lipo ", "xcodebuild "];

fn ios_slices() -> String {
    format!("{HOST_ROOT}/{BUILD_ROOT}/ios/{SLICES_DIR}")
}

fn copies(plan: &[Step]) -> Vec<(String, String)> {
    plan.iter()
        .filter_map(|step| match step {
            Step::Copy { from, to } => Some((
                from.to_string_lossy().to_string(),
                to.to_string_lossy().to_string(),
            )),
            _ => None,
        })
        .collect()
}

fn after_the_toolchain(commands: Vec<String>) -> Vec<String> {
    let first = commands
        .iter()
        .rposition(|command| command.starts_with("rustup"))
        .map_or(0, |at| at + 1);
    commands[first..].to_vec()
}

#[test]
fn the_ios_slice_plan_builds_one_triple_and_delivers_its_archive() -> Result<(), Vec<String>> {
    for target in iter_ios_targets() {
        let triple = target.triple();
        let plan = ios_plan(&host_roots()?, IosPhase::Slice(target));
        let commands = commands(&plan);
        let target_add =
            workbench::required(position_of(&commands, TARGET_ADD), "target add step")?;
        assert_eq!(commands[target_add], format!("{TARGET_ADD} {triple}"));
        let builds: Vec<&String> = commands
            .iter()
            .filter(|command| command.starts_with(CARGO_BUILD))
            .collect();
        let [build] = builds[..] else {
            return Err(vec![format!(
                "one cargo build per slice, found {}",
                builds.len()
            )]);
        };
        assert!(build.contains(&format!("--target {triple}")));
        assert!(
            build.ends_with(&binding_layer::PROXY_FEATURE_ARGS.join(" ")),
            "{build} leaves the proxy crate out of the archive"
        );
        assert!(commands.iter().all(|command| {
            !command.contains(LIBRARY_FLAG)
                && ASSEMBLY_PROGRAMS
                    .iter()
                    .all(|program| !command.starts_with(program))
        }));
        let delivered: Vec<String> = copies(&plan).into_iter().map(|(_, to)| to).collect();
        assert_eq!(
            delivered,
            vec![format!(
                "{}/{triple}/{}",
                ios_slices(),
                ios_static_library(binding_layer::WALLET_LIB_NAME)
            )]
        );
        let describes = plan
            .iter()
            .filter(|step| matches!(step, Step::Describe { .. }))
            .count();
        assert_eq!(describes, usize::from(target == IosTarget::Device));
        assert!(!matches!(plan.last(), Some(Step::XcodeTest { .. })));
    }
    Ok(())
}

#[test]
fn the_ios_slice_plan_leaves_the_package_output_alone() -> Result<(), Vec<String>> {
    let roots = host_roots()?;
    let output = format!("{}/", roots.out_host.to_string_lossy());
    for target in iter_ios_targets() {
        for step in ios_plan(&roots, IosPhase::Slice(target)) {
            if let Step::FreshDir(dir) | Step::Remove(dir) = step {
                let dir = dir.to_string_lossy();
                assert!(
                    !dir.starts_with(&output) && dir != roots.out_host.to_string_lossy(),
                    "a slice clears {dir}"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn the_ios_assembly_plan_requires_every_slice_before_it_reads_one() -> Result<(), Vec<String>> {
    let plan = ios_plan(&host_roots()?, IosPhase::Assembly);
    let required: Vec<String> = plan
        .iter()
        .filter_map(|step| match step {
            Step::Require(file) => Some(file.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    let mut expected = Vec::new();
    for target in iter_ios_targets() {
        expected.push(format!(
            "{}/{}/{}",
            ios_slices(),
            target.triple(),
            ios_static_library(binding_layer::WALLET_LIB_NAME)
        ));
        if target == IosTarget::Device {
            expected.push(format!(
                "{}/{}/{}",
                ios_slices(),
                target.triple(),
                binding_layer::DESCRIPTOR_FILE
            ));
        }
    }
    assert_eq!(required, expected);
    let last_require = workbench::required(
        plan.iter()
            .rposition(|step| matches!(step, Step::Require(_))),
        "a require step",
    )?;
    let first_read = workbench::required(
        plan.iter().position(|step| {
            matches!(step, Step::Run { command, .. } if !command[0].starts_with("rustup"))
                || matches!(step, Step::Copy { .. })
        }),
        "a step that reads a slice",
    )?;
    assert!(
        last_require < first_read,
        "a slice is read before every slice is required"
    );
    Ok(())
}

#[test]
fn the_ios_assembly_plan_compiles_no_target_and_reads_only_the_slices() -> Result<(), Vec<String>> {
    let plan = ios_plan(&host_roots()?, IosPhase::Assembly);
    let commands = commands(&plan);
    assert!(position_of(&commands, TARGET_ADD).is_none());
    assert!(commands
        .iter()
        .all(|command| !command.starts_with(CARGO_BUILD)));
    for generation in binding_layer::GENERATIONS {
        for target in iter_ios_targets() {
            let built = format!(
                "{HOST_ROOT}/{BUILD_ROOT}/ios/{}/{}",
                generation.names().label,
                target.triple()
            );
            assert!(
                commands.iter().all(|command| !command.contains(&built)),
                "a command reads {built}"
            );
        }
    }
    assert!(plan
        .iter()
        .all(|step| !matches!(step, Step::Describe { .. })));
    let descriptor = (
        format!(
            "{}/{}/{}",
            ios_slices(),
            IosTarget::Device.triple(),
            binding_layer::DESCRIPTOR_FILE
        ),
        format!("{OUTSIDE_OUT}/{}", binding_layer::DESCRIPTOR_FILE),
    );
    assert!(copies(&plan).contains(&descriptor));
    assert!(matches!(plan.last(), Some(Step::XcodeTest { .. })));
    Ok(())
}

fn rendered(plan: &[Step]) -> Vec<String> {
    let paths = |paths: &[path::PathBuf]| {
        paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    };
    plan.iter()
        .map(|step| match step {
            Step::Run { command, .. } => command.join(" "),
            Step::Copy { from, to } => format!("copy {}", paths(&[from.clone(), to.clone()])),
            Step::Concatenate { sources, to, .. } => {
                format!("concatenate {} {}", paths(sources), to.to_string_lossy())
            }
            Step::FreshDir(dir) => format!("fresh {}", dir.to_string_lossy()),
            Step::Remove(dir) => format!("remove {}", dir.to_string_lossy()),
            Step::Require(file) => format!("require {}", file.to_string_lossy()),
            Step::Describe { command, to, .. } => {
                format!("describe {} {}", command.join(" "), to.to_string_lossy())
            }
            Step::XcodeTest { workdir, scheme } => format!("xcodetest {workdir} {scheme}"),
        })
        .collect()
}

#[test]
fn the_full_ios_plan_is_every_slice_plan_followed_by_the_assembly() -> Result<(), Vec<String>> {
    let roots = host_roots()?;
    let mut expected = Vec::new();
    for target in iter_ios_targets() {
        expected.extend(after_the_toolchain(rendered(&ios_plan(
            &roots,
            IosPhase::Slice(target),
        ))));
    }
    expected.extend(after_the_toolchain(rendered(&ios_plan(
        &roots,
        IosPhase::Assembly,
    ))));
    assert_eq!(
        after_the_toolchain(rendered(&ios_plan(&roots, IosPhase::Full))),
        expected
    );
    Ok(())
}

#[test]
fn the_ios_flags_select_one_slice_or_the_assembly() -> Result<(), Vec<String>> {
    let simulator = IosTarget::Simulator(IosArch::Arm64);
    let Arguments { platform, .. } =
        parse(&["ios", TARGET_FLAG, simulator.triple()].map(String::from))?;
    assert!(matches!(platform, Platform::Ios(IosPhase::Slice(target)) if target == simulator));
    let Arguments { platform, .. } = parse(&["ios", ASSEMBLE_FLAG].map(String::from))?;
    assert!(matches!(platform, Platform::Ios(IosPhase::Assembly)));
    let both = workbench::refused(parse(
        &["ios", TARGET_FLAG, simulator.triple(), ASSEMBLE_FLAG].map(String::from),
    ))?
    .concat();
    assert!(both.contains(TARGET_FLAG) && both.contains(ASSEMBLE_FLAG));
    let android_triple = binding_layer::ANDROID_ABIS[FIRST_POSITION].triple;
    let unknown = workbench::refused(parse(
        &["ios", TARGET_FLAG, android_triple].map(String::from),
    ))?
    .concat();
    assert!(unknown.contains(android_triple));
    for target in iter_ios_targets() {
        assert!(unknown.contains(target.triple()), "{unknown}");
    }
    assert!(unknown.contains(USAGE), "{unknown}");
    for other in ["android", "kotlin"] {
        let diagnostic = workbench::refused(parse(
            &[other, ASSEMBLE_FLAG, OUT_FLAG, "out"].map(String::from),
        ))?
        .concat();
        assert!(diagnostic.contains(ASSEMBLE_FLAG));
    }
    Ok(())
}

#[test]
fn the_ios_workflow_pins_one_xcode_and_keys_its_caches_by_it() -> Result<(), Vec<String>> {
    let workflow = workbench::read(&workbench::repo_root()?.join(IOS_WORKFLOW))?;
    let lines: Vec<&str> = workflow.lines().map(str::trim).collect();
    let pins: Vec<&str> = lines
        .iter()
        .filter(|line| line.starts_with(XCODE_VERSION_KEY))
        .map(|line| {
            line.trim_start_matches(XCODE_VERSION_KEY)
                .trim()
                .trim_matches('\'')
        })
        .collect();
    let [version] = pins[..] else {
        return Err(vec![format!(
            "{IOS_WORKFLOW} pins {} Xcode versions, not one",
            pins.len()
        )]);
    };
    assert!(
        version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
        "`{version}` is no explicit Xcode version"
    );
    assert_eq!(version, binding_layer::IOS_XCODE_VERSION);
    let selections: Vec<&&str> = lines
        .iter()
        .filter(|line| line.starts_with(XCODE_VERSION_INPUT))
        .collect();
    assert!(!selections.is_empty());
    for selection in selections {
        assert_eq!(
            *selection,
            format!("{XCODE_VERSION_INPUT} {XCODE_VERSION_REFERENCE}")
        );
    }
    let keys: Vec<&&str> = lines
        .iter()
        .filter(|line| line.starts_with(IOS_CACHE_KEY) || line.starts_with(IOS_RESTORE_KEY))
        .collect();
    assert_eq!(keys.len(), 6);
    for key in keys {
        assert!(
            key.contains(XCODE_VERSION_REFERENCE),
            "`{key}` is not keyed by the Xcode version"
        );
    }
    Ok(())
}

#[test]
fn the_ios_workflow_matrix_names_every_ios_target() -> Result<(), Vec<String>> {
    let workflow = workbench::read(&workbench::repo_root()?.join(IOS_WORKFLOW))?;
    let line = workflow
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(MATRIX_TARGETS_KEY))
        .ok_or_else(|| vec![format!("{IOS_WORKFLOW} lists no matrix targets")])?;
    let listed: Vec<&str> = line
        .trim_start_matches(MATRIX_TARGETS_KEY)
        .trim_end_matches(']')
        .split(',')
        .map(str::trim)
        .collect();
    let known: Vec<&str> = iter_ios_targets().map(|target| target.triple()).collect();
    assert_eq!(listed, known);
    Ok(())
}

const OUTPUT_FLAGS: [&str; 2] = ["-o", "-output"];

fn written_paths(step: &Step) -> Vec<String> {
    match step {
        Step::Run { command, .. } => command
            .windows(2)
            .filter(|pair| OUTPUT_FLAGS.contains(&pair[0].as_str()))
            .map(|pair| pair[1].clone())
            .collect(),
        Step::Copy { to, .. } | Step::Concatenate { to, .. } | Step::Describe { to, .. } => {
            vec![to.to_string_lossy().to_string()]
        }
        Step::FreshDir(_) | Step::Remove(_) | Step::Require(_) | Step::XcodeTest { .. } => {
            vec![]
        }
    }
}

#[test]
fn no_plan_recreates_a_directory_an_earlier_step_wrote_into() -> Result<(), Vec<String>> {
    let roots = host_roots()?;
    let plans = [
        ios_plan(&roots, IosPhase::Full),
        ios_plan(&roots, IosPhase::Assembly),
    ];
    for plan in plans.iter().chain(every_plan()?.iter()) {
        let mut written: Vec<String> = Vec::new();
        for step in plan {
            if let Step::FreshDir(dir) = step {
                let dir = format!("{}/", dir.to_string_lossy());
                let lost: Vec<&String> = written
                    .iter()
                    .filter(|path| path.starts_with(&dir))
                    .collect();
                assert!(lost.is_empty(), "recreating {dir} discards {lost:?}");
            }
            written.extend(written_paths(step));
        }
    }
    Ok(())
}

#[test]
fn the_ios_plan_takes_no_output_directory() -> Result<(), Vec<String>> {
    let Arguments { platform, out, .. } = parse(&["ios".to_string()])?;
    assert!(matches!(platform, Platform::Ios(IosPhase::Full)));
    assert_eq!(out, None);
    let diagnostic =
        workbench::refused(parse(&["ios", OUT_FLAG, "out"].map(String::from)))?.concat();
    assert!(diagnostic.contains(OUT_FLAG));
    Ok(())
}

#[test]
fn the_other_plans_require_an_output_directory() -> Result<(), Vec<String>> {
    for platform in ["android", "kotlin"] {
        let Arguments { out, .. } = parse(&[platform, OUT_FLAG, "out"].map(String::from))?;
        assert!(out.is_some_and(|out| out.ends_with("out")));
        let Arguments { out, .. } = parse(&[platform.to_string()])?;
        let diagnostic = workbench::refused(required_out(out))?.concat();
        assert!(diagnostic.contains(OUT_FLAG));
    }
    Ok(())
}

#[test]
fn the_xcodebuild_test_names_the_scheme_and_the_simulator_by_id() {
    assert_eq!(
        xcodebuild_test("Scheme-Package", "5555").join(" "),
        "xcodebuild test -scheme Scheme-Package -destination platform=iOS Simulator,id=5555,arch=arm64"
    );
}

#[test]
fn kotlin_plan_adds_no_target() -> Result<(), Vec<String>> {
    assert!(position_of(&commands(&kotlin_plan(&host_roots()?)), TARGET_ADD).is_none());
    Ok(())
}

#[test]
fn no_plan_step_overrides_the_toolchain_pin() -> Result<(), Vec<String>> {
    for plan in every_plan()? {
        assert!(plan.iter().all(|step| match step {
            Step::Run { env, .. } | Step::Describe { env, .. } => env
                .iter()
                .all(|(key, _)| key != binding_layer::TOOLCHAIN_VARIABLE),
            _ => true,
        }));
    }
    Ok(())
}

#[test]
fn the_host_command_clears_the_toolchain_override_it_inherits() {
    let started = invocation(
        &Runner::Host,
        HOST_ROOT,
        &[("CC".to_string(), "clang".to_string())],
        &["cargo".to_string(), "build".to_string()],
    );
    let command = host_command(&started);
    let envs: Vec<_> = command.get_envs().collect();
    assert!(envs.contains(&(
        std::ffi::OsStr::new(binding_layer::TOOLCHAIN_VARIABLE),
        None
    )));
    assert!(envs.contains(&(
        std::ffi::OsStr::new("CC"),
        Some(std::ffi::OsStr::new("clang"))
    )));
}

#[test]
fn the_container_command_clears_the_toolchain_override_the_image_carries() -> Result<(), Vec<String>>
{
    let started = invocation(
        &Runner::Container {
            engine: binding_layer::ENGINES[FIRST_POSITION],
            id: CONTAINER_ID.to_string(),
        },
        CONTAINER_ROOT,
        &[("CC".to_string(), "clang".to_string())],
        &["cargo".to_string(), "build".to_string()],
    );
    let cleared = format!("{}=", binding_layer::TOOLCHAIN_VARIABLE);
    let id = workbench::required(
        started.args.iter().position(|arg| arg == CONTAINER_ID),
        "container id argument",
    )?;
    assert!(started.args[..id]
        .windows(2)
        .any(|pair| pair[0] == "--env" && pair[1] == cleared));
    Ok(())
}

#[test]
fn every_android_run_step_carries_the_base_environment() -> Result<(), Vec<String>> {
    let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
    let plan = android_plan(&roots()?, &abis);
    let base = android_base_env();
    assert!(plan.iter().all(|step| match step {
        Step::Run { env, .. } | Step::Describe { env, .. } => {
            base.iter().all(|entry| env.contains(entry))
        }
        _ => true,
    }));
    Ok(())
}

#[test]
fn the_android_and_ios_plans_describe_the_wallet_build_with_cargo_messages(
) -> Result<(), Vec<String>> {
    let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
    for plan in [
        android_plan(&roots()?, &abis),
        ios_plan(&host_roots()?, IosPhase::Full),
    ] {
        let describes: Vec<_> = plan
            .iter()
            .filter_map(|step| match step {
                Step::Describe {
                    workdir,
                    ndk,
                    command,
                    to,
                    ..
                } => Some((workdir, ndk, command, to)),
                _ => None,
            })
            .collect();
        let [(workdir, ndk, command, to)] = describes[..] else {
            return Err(vec![format!(
                "one describe step per plan, found {}",
                describes.len()
            )]);
        };
        assert!(workdir.ends_with(binding_layer::WALLET_CRATE_DIR));
        assert_eq!(
            &command[..3],
            ["cargo", "build", "--target"].map(String::from)
        );
        assert!(command.iter().all(|word| word != "ndk"));
        assert_eq!(
            command.last().map(String::as_str),
            Some(binding_layer::MESSAGE_FORMAT_FLAG)
        );
        assert!(to.ends_with(binding_layer::DESCRIPTOR_FILE));
        match ndk {
            Some(link) => {
                assert_eq!(command[3], abis[FIRST_POSITION].triple);
                assert_eq!(
                    link.env_command,
                    binding_layer::ndk_env_command(&command[3])
                );
            }
            None => {
                assert_eq!(command[3], IosTarget::Device.triple());
                let described = command[..command.len() - 1].join(" ");
                assert!(
                    commands(&plan).contains(&described),
                    "the describe step builds `{described}`, which no slice build does"
                );
            }
        }
    }
    Ok(())
}

/// The zingolib root as the host sees it in tests.
const HOST_ROOT: &str = "/host/zingolib";

/// An output directory outside the zingolib root.
const OUTSIDE_OUT: &str = "/host/consumer/android/build/binding-layer";

/// An output directory inside the zingolib root.
const INSIDE_OUT: &str = "/host/zingolib/bindings/android/build/binding-layer";

/// The output directory that the Android workflow names, relative to the root.
const RELATIVE_OUT: &str = "bindings/android/build/binding-layer";

fn roots() -> Result<Roots, Vec<String>> {
    Roots::in_container(
        path::PathBuf::from(HOST_ROOT),
        path::PathBuf::from(INSIDE_OUT),
    )
}

fn commands(plan: &[Step]) -> Vec<String> {
    plan.iter()
        .filter_map(|step| match step {
            Step::Run { command, .. } => Some(command.join(" ")),
            _ => None,
        })
        .collect()
}

#[test]
fn android_plan_generates_each_binding_set_once_from_the_first_abi() -> Result<(), Vec<String>> {
    let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
    let plan = android_plan(&roots()?, &abis);
    let generations = commands(&plan)
        .into_iter()
        .filter(|command| command.contains("--library"))
        .count();
    assert_eq!(generations, GENERATIONS_PER_BUILD);
    Ok(())
}

#[test]
fn every_bindgen_takes_the_aarch64_environment_whatever_the_selection() -> Result<(), Vec<String>> {
    let x86: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS
        .iter()
        .filter(|abi| abi.jni_dir == "x86")
        .collect();
    let plan = android_plan(&roots()?, &x86);
    let expected_cc = binding_layer::ANDROID_ABIS[FIRST_POSITION].cc();
    let bindgen_envs: Vec<_> = plan
        .iter()
        .filter_map(|step| match step {
            Step::Run { env, command, .. } if command.iter().any(|arg| arg == "--library") => {
                Some(env.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(bindgen_envs.len(), GENERATIONS_PER_BUILD);
    assert!(bindgen_envs.iter().all(|env| env
        .iter()
        .any(|(key, value)| key == "CC" && *value == expected_cc)));
    Ok(())
}

#[test]
fn kotlin_plan_generates_each_set_after_its_own_host_build() -> Result<(), Vec<String>> {
    let plan = kotlin_plan(&host_roots()?);
    let commands = commands(&plan);
    assert!(
        matches!(plan.first(), Some(Step::FreshDir(out)) if out == path::Path::new(OUTSIDE_OUT))
    );
    assert!(commands.iter().all(|command| !command.contains(" ndk ")));
    let position = |predicate: fn(&String) -> bool, what: &str| {
        workbench::required(commands.iter().position(predicate), what)
    };
    let wallet_build = position(
        |command| {
            command.starts_with("cargo build") && !command.contains(binding_layer::PROXY_PACKAGE)
        },
        "wallet build step",
    )?;
    let wallet_generation = position(
        |command| {
            command.contains(&binding_layer::host_library_file(
                binding_layer::WALLET_LIB_NAME,
            ))
        },
        "wallet generation step",
    )?;
    let proxy_build = position(
        |command| {
            command.starts_with("cargo build") && command.contains(binding_layer::PROXY_PACKAGE)
        },
        "proxy build step",
    )?;
    let proxy_generation = position(
        |command| {
            command.contains(&binding_layer::host_library_file(
                binding_layer::PROXY_LIB_NAME,
            ))
        },
        "proxy generation step",
    )?;
    assert!(wallet_build < wallet_generation);
    assert!(wallet_generation < proxy_build);
    assert!(proxy_build < proxy_generation);
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.contains("--language kotlin"))
            .count(),
        binding_layer::GENERATIONS.len()
    );
    Ok(())
}

#[test]
fn kotlin_bindgens_read_the_libraries_the_host_builds_write() -> Result<(), Vec<String>> {
    let plan = kotlin_plan(&host_roots()?);
    let build_targets: Vec<_> = plan
        .iter()
        .filter_map(|step| match step {
            Step::Run { env, command, .. } if command.join(" ").starts_with("cargo build") => env
                .iter()
                .find(|(key, _)| key == binding_layer::TARGET_DIR_VARIABLE)
                .map(|(_, target)| target.clone()),
            _ => None,
        })
        .collect();
    let libraries: Vec<_> = plan
        .iter()
        .filter_map(|step| match step {
            Step::Run { command, .. } => command
                .iter()
                .position(|arg| arg == "--library")
                .map(|flag| command[flag + 1].clone()),
            _ => None,
        })
        .collect();
    let lib_names = [
        binding_layer::WALLET_LIB_NAME,
        binding_layer::PROXY_LIB_NAME,
    ];
    assert_eq!(build_targets.len(), lib_names.len());
    assert_eq!(libraries.len(), lib_names.len());
    for ((build_target, library), lib_name) in build_targets.iter().zip(&libraries).zip(lib_names) {
        assert_eq!(
            *library,
            format!(
                "{build_target}/{}/{}",
                HOST_PROFILE.directory(),
                binding_layer::host_library_file(lib_name)
            )
        );
    }
    assert!(commands(&plan)
        .iter()
        .all(|command| !command.contains("--release")));
    Ok(())
}

/// Tests that every Kotlin generation runs from the standalone bindgen package, whose only dependency is uniffi.
#[test]
fn kotlin_plan_runs_every_bindgen_from_the_standalone_package() -> Result<(), Vec<String>> {
    let generations = commands(&kotlin_plan(&host_roots()?))
        .into_iter()
        .filter(|command| command.contains("--language kotlin"))
        .collect::<Vec<_>>();
    assert_eq!(generations.len(), binding_layer::GENERATIONS.len());
    assert!(generations
        .iter()
        .all(|command| command.contains("--package zingolib-uniffi-bindgen")));
    assert!(generations.iter().all(
        |command| !command.contains(&format!("{}/{MANIFEST}", binding_layer::WALLET_CRATE_DIR))
    ));
    Ok(())
}

#[test]
fn abi_is_refused_off_android() {
    let args = |platform| {
        [platform, ABI_FLAG, "x86", OUT_FLAG, "out"]
            .map(String::from)
            .to_vec()
    };
    assert!(parse(&args("kotlin")).is_err());
    assert!(parse(&args("ios")).is_err());
    assert!(parse(&args("android")).is_ok());
}

#[test]
fn every_android_library_builds_under_the_one_mobile_profile() -> Result<(), Vec<String>> {
    let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
    let builds: Vec<String> = android_plan(&roots()?, &abis)
        .iter()
        .filter_map(|step| match step {
            Step::Run { command, .. } if command.iter().any(|arg| arg == "build") => {
                Some(command.join(" "))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        builds.len(),
        abis.len() * binding_layer::BINDING_CRATE_DIRS.len(),
        "{builds:?}"
    );
    let profile = BUILDER_PROFILE.cargo_args().join(" ");
    assert!(
        builds.iter().all(|command| command.contains(&profile)),
        "a library builds under another profile than {profile}: {builds:?}"
    );
    Ok(())
}

#[test]
fn every_android_library_builds_into_one_target_directory() -> Result<(), Vec<String>> {
    let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
    let mut targets: Vec<String> = android_plan(&roots()?, &abis)
        .iter()
        .filter_map(|step| match step {
            Step::Run { env, command, .. } if command.iter().any(|arg| arg == "build") => env
                .iter()
                .find(|(key, _)| key == binding_layer::TARGET_DIR_VARIABLE)
                .map(|(_, target)| target.clone()),
            _ => None,
        })
        .collect();
    targets.dedup();
    assert_eq!(
        targets.len(),
        1,
        "the wallet and the proxy build into separate target directories, and every dependency they share compiles twice per ABI: {targets:?}"
    );
    Ok(())
}

#[test]
fn android_plan_copies_both_libraries_for_every_abi() -> Result<(), Vec<String>> {
    let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
    let plan = android_plan(&roots()?, &abis);
    let copies = plan
        .iter()
        .filter(|step| matches!(step, Step::Copy { .. }))
        .count();
    assert_eq!(
        copies,
        binding_layer::ANDROID_ABIS.len() * binding_layer::GENERATIONS.len()
    );
    Ok(())
}

#[test]
fn the_container_refuses_an_output_directory_outside_the_mounted_root() -> Result<(), Vec<String>> {
    let diagnostic = workbench::refused(Roots::in_container(
        path::PathBuf::from(HOST_ROOT),
        path::PathBuf::from(OUTSIDE_OUT),
    ))?
    .concat();
    assert!(diagnostic.contains(OUTSIDE_OUT) && diagnostic.contains("mounts"));
    Ok(())
}

#[test]
fn the_container_sees_the_output_directory_under_its_own_root() -> Result<(), Vec<String>> {
    assert_eq!(
        roots()?.out_run_path(binding_layer::KOTLIN_OUT_DIR),
        format!(
            "{CONTAINER_ROOT}/bindings/android/build/binding-layer/{}",
            binding_layer::KOTLIN_OUT_DIR
        )
    );
    Ok(())
}

#[test]
fn the_container_sees_the_output_directory_with_slashes_whatever_the_host_separator(
) -> Result<(), Vec<String>> {
    let root = path::PathBuf::from(HOST_ROOT);
    let out = root
        .join("bindings")
        .join("android")
        .join("build")
        .join("binding-layer");
    assert_eq!(
        Roots::in_container(root, out)?.out_run,
        format!("{CONTAINER_ROOT}/{RELATIVE_OUT}")
    );
    Ok(())
}

#[test]
fn the_host_takes_an_output_directory_outside_the_root() -> Result<(), Vec<String>> {
    let plan = ios_plan(&host_roots()?, IosPhase::Full);
    assert!(matches!(
        plan.first(),
        Some(Step::FreshDir(directory)) if directory == path::Path::new(OUTSIDE_OUT)
    ));
    assert!(commands(&plan)
        .iter()
        .any(|command| command.contains(&format!("{OUTSIDE_OUT}/{GENERATED_DIR}/wallet"))));
    Ok(())
}

/// A container engine that no host has installed.
const ABSENT_ENGINE: &str = "absent-container-engine";

#[test]
fn a_container_engine_that_does_not_start_is_reported_by_name_alone() -> Result<(), Vec<String>> {
    let runner = Runner::Container {
        engine: ABSENT_ENGINE,
        id: "container".to_string(),
    };
    let step_workdir = format!("{CONTAINER_ROOT}/{}", binding_layer::WALLET_CRATE_DIR);
    let diagnostic = workbench::refused(run_command(
        &runner,
        &step_workdir,
        &[],
        &["cargo".to_string()],
    ))?
    .concat();
    assert!(diagnostic.contains(ABSENT_ENGINE) && !diagnostic.contains(&step_workdir));
    Ok(())
}

#[test]
fn an_absent_host_working_directory_is_reported_before_the_program() -> Result<(), Vec<String>> {
    let step_workdir = format!("{HOST_ROOT}/{}", binding_layer::WALLET_CRATE_DIR);
    let diagnostic = workbench::refused(run_command(
        &Runner::Host,
        &step_workdir,
        &[],
        &["cargo".to_string()],
    ))?
    .concat();
    assert!(diagnostic.contains(&step_workdir) && !diagnostic.contains("cargo"));
    let captured = workbench::refused(capture_command(
        &Runner::Host,
        &step_workdir,
        &[],
        &["cargo".to_string()],
    ))?
    .concat();
    assert_eq!(captured, diagnostic);
    Ok(())
}

#[test]
fn an_output_directory_holding_the_root_is_refused_on_both_runners() -> Result<(), Vec<String>> {
    let root = workbench::repo_root()?;
    let parent = workbench::required(root.parent(), "parent of the repository root")?.to_path_buf();
    assert!(Roots::on_host(root.clone(), root.clone()).is_err());
    assert!(Roots::on_host(root.clone(), parent.clone()).is_err());
    assert!(Roots::in_container(root.clone(), root.clone()).is_err());
    assert!(Roots::in_container(root.clone(), parent).is_err());
    assert!(Roots::on_host(root.clone(), root.join(RELATIVE_OUT)).is_ok());
    Ok(())
}

#[test]
fn host_of_maps_container_paths_back_to_the_host() -> Result<(), Vec<String>> {
    assert_eq!(
        host_of(&roots()?, "/opt/zingolib/target/x.so"),
        path::PathBuf::from("/host/zingolib/target/x.so")
    );
    Ok(())
}
