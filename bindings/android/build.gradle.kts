plugins {
    id("com.android.library") version "8.11.0"
    id("org.jetbrains.kotlin.android") version "2.1.20"
}

val bindingCompileSdk = 36
val bindingMinSdk = 26
val bindingNdkVersion = "28.2.13676358"
val jvmTargetVersion = JavaVersion.VERSION_17
val jnaDependency = "net.java.dev.jna:jna:5.18.1@aar"
val annotationDependency = "androidx.annotation:annotation:1.8.1"
val releaseTagPrefix = "zingo-"

group = "org.zingolabs"

val zingolibRoot: File = rootDir.resolve("../..")
val builderOutput: Provider<Directory> = layout.buildDirectory.dir("binding-layer")
val prebuiltOutput: Provider<Directory> =
    layout.dir(providers.gradleProperty("bindingLayerPrebuilt").map { file(it) })
val layerOutput: Provider<Directory> = prebuiltOutput.orElse(builderOutput)
// The consumer's descriptor from its own checkout: `zm_<tag#>` on a release
// tag, else `zm_<hash5>`. A tag that points at HEAD needs no history.
val consumerDescriptor: Provider<String> = gradle.parent?.let { consumer ->
    val consumerDir = consumer.startParameter.currentDir
    val releaseTag = providers.exec {
        workingDir = consumerDir
        commandLine(
            "git", "tag", "--points-at", "HEAD", "--list", "$releaseTagPrefix*", "--sort=-version:refname",
        )
    }.standardOutput.asText.map { it.trim().lineSequence().first() }
    val hash5 = providers.exec {
        workingDir = consumerDir
        commandLine("git", "rev-parse", "HEAD")
    }.standardOutput.asText.map { it.trim().take(5) }
    val dirty = providers.exec {
        workingDir = consumerDir
        isIgnoreExitValue = true
        commandLine("git", "diff-index", "--quiet", "HEAD", "--")
    }.result.map { it.exitValue != 0 }
    releaseTag.zip(hash5) { tag, hash ->
        if (tag.isEmpty()) "zm_$hash" else "zm_" + tag.removePrefix(releaseTagPrefix)
    }.zip(dirty) { descriptor, isDirty ->
        if (isDirty) "${descriptor}_dirty" else descriptor
    }
} ?: providers.provider<String> { null }
val zingoMobileDescriptor: Provider<String> = providers.environmentVariable("ZINGO_MOBILE_DESCRIPTOR")
    .orElse(providers.gradleProperty("zingoMobileDescriptor"))
    .orElse(consumerDescriptor)
val selectedAbi: Provider<String> = providers.gradleProperty("bindingLayerAbi")

val buildBindingLayer by tasks.registering(Exec::class) {
    description = "Builds the Binding Layer's Android libraries and Kotlin sources."
    onlyIf("no prebuilt Binding Layer is named") { !prebuiltOutput.isPresent }
    workingDir = zingolibRoot
    doFirst {
        environment("ZINGO_MOBILE_DESCRIPTOR", zingoMobileDescriptor.getOrElse(""))
    }
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
            exclude("**/target/**", "**/build/**", ".git/**", "bindings/swift/**")
        }
    )
    inputs.property("zingoMobileDescriptor", zingoMobileDescriptor.orElse(""))
    inputs.property("selectedAbi", selectedAbi.getOrElse(""))
    outputs.dir(builderOutput)
}

android {
    namespace = "org.zingolabs.bindinglayer"
    compileSdk = bindingCompileSdk
    ndkVersion = bindingNdkVersion

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

dependencies {
    api(jnaDependency)
    compileOnly(annotationDependency)
}
