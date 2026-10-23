plugins {
    id("com.android.library") version "8.11.0"
    id("org.jetbrains.kotlin.android") version "2.1.20"
    id("maven-publish")
}

val bindingCompileSdk = 36
val bindingMinSdk = 26
val jvmTargetVersion = JavaVersion.VERSION_17
val jnaDependency = "net.java.dev.jna:jna:5.18.1@aar"
val annotationDependency = "androidx.annotation:annotation:1.8.1"
val junitDependency = "junit:junit:4.13.2"

val bindingLayerVersion: Provider<String> = providers.gradleProperty("bindingLayerVersion")
val unversioned = "unversioned"
val repositoryOutput: Provider<Directory> = layout.buildDirectory.dir("repository")

group = "org.zingolabs"
version = bindingLayerVersion.getOrElse(unversioned)

val zingolibRoot: File = rootDir.resolve("../..")
val proxyCrate: File = zingolibRoot.resolve("zingo-netutils/nym-proxy-ffi")
val contractTests: File = proxyCrate.resolve("contract-tests/kotlin")
val goldenDir: File = proxyCrate.resolve("test-data/golden")
val testAnnotation = Regex("""@Test\b""")

listOf(contractTests, goldenDir).forEach { directory ->
    if (!directory.isDirectory) {
        throw GradleException("the Kotlin wire-contract test reads ${directory.normalize()}, which is not a directory")
    }
}
val contractTestCount: Int = contractTests.walk()
    .filter { it.extension == "kt" }
    .sumOf { testAnnotation.findAll(it.readText()).count() }
if (contractTestCount == 0) {
    throw GradleException("no @Test method under ${contractTests.normalize()}")
}

val builderOutput: Provider<Directory> = layout.buildDirectory.dir("binding-layer")
val prebuiltOutput: Provider<Directory> =
    layout.dir(providers.gradleProperty("bindingLayerPrebuilt").map { file(it) })
val layerOutput: Provider<Directory> = prebuiltOutput.orElse(builderOutput)
val selectedAbi: Provider<String> = providers.gradleProperty("bindingLayerAbi")

val buildBindingLayer by tasks.registering(Exec::class) {
    description = "Builds the Binding Layer's Android libraries and Kotlin sources."
    onlyIf("no prebuilt Binding Layer is named") { !prebuiltOutput.isPresent }
    workingDir = zingolibRoot
    commandLine(
        listOf(
            "cargo", "run", "--quiet",
            "--manifest-path", "tools/workbench/Cargo.toml",
            "--bin", "build-binding-layer", "--",
            "android", "--out", builderOutput.get().asFile.absolutePath,
        ) + selectedAbi.map { listOf("--abi", it) }.getOrElse(emptyList())
    )
    inputs.files(
        fileTree(zingolibRoot) {
            exclude("**/target/**", "**/build/**", "**/.gradle/**", "**/.kotlin/**", ".git/**", "bindings/swift/**")
        }
    )
    inputs.property("selectedAbi", selectedAbi.getOrElse(""))
    outputs.dir(builderOutput)
}

android {
    namespace = "org.zingolabs.bindinglayer"
    compileSdk = bindingCompileSdk

    defaultConfig {
        minSdk = bindingMinSdk
    }

    compileOptions {
        sourceCompatibility = jvmTargetVersion
        targetCompatibility = jvmTargetVersion
    }

    sourceSets {
        getByName("main") {
            java.srcDir(layerOutput.map { it.dir("kotlin") })
            jniLibs.srcDir(layerOutput.map { it.dir("jniLibs") })
        }
        getByName("test") {
            java.srcDir(contractTests)
        }
    }

    testOptions {
        unitTests.all {
            it.systemProperty("zingo.golden.dir", goldenDir.normalize().absolutePath)
        }
    }

    packaging {
        jniLibs {
            keepDebugSymbols.add("**/*.so")
        }
    }

    publishing {
        singleVariant("release")
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.fromTarget(jvmTargetVersion.toString()))
    }
}

tasks.named("preBuild") {
    dependsOn(buildBindingLayer)
}

val verifyContractTestsRan by tasks.registering {
    description = "Fails when a unit-test task ran no tests."
    val unitTests = tasks.withType<Test>()
    doLast {
        unitTests.firstOrNull { it.state.executed && !(it.state.didWork || it.state.upToDate) }?.let { idle ->
            throw GradleException("${idle.name} ran no tests, and the Kotlin wire-contract suite holds $contractTestCount")
        }
    }
}

tasks.withType<Test>().configureEach {
    afterSuite(KotlinClosure2<TestDescriptor, TestResult, Unit>({ suite, result ->
        if (suite.parent == null && result.testCount != contractTestCount.toLong()) {
            throw GradleException("$name executed ${result.testCount} tests, and the Kotlin wire-contract suite holds $contractTestCount")
        }
    }))
    finalizedBy(verifyContractTestsRan)
}

val clearRepository by tasks.registering(Delete::class) {
    description = "Clears the Maven repository directory before a publication writes it."
    delete(repositoryOutput)
}

afterEvaluate {
    publishing {
        publications {
            register<MavenPublication>("release") {
                from(components["release"])
            }
        }
        repositories {
            maven {
                name = "bundle"
                url = uri(repositoryOutput)
            }
        }
    }
}

tasks.withType<PublishToMavenRepository>().configureEach {
    dependsOn(clearRepository)
    doFirst {
        if (!bindingLayerVersion.isPresent) {
            throw GradleException(
                "Set the Gradle property bindingLayerVersion from `build-binding-layer artifact` before publishing."
            )
        }
    }
}

dependencies {
    api(jnaDependency)
    compileOnly(annotationDependency)
    testImplementation(junitDependency)
}
