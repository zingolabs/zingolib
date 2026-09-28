plugins {
    id("com.android.library") version "8.11.0"
    id("org.jetbrains.kotlin.android") version "2.1.20"
}

val bindingCompileSdk = 36
val bindingMinSdk = 26
val bindingNdkVersion = "28.2.13676358"
val jvmTargetVersion = JavaVersion.VERSION_17
val jnaDependency = "net.java.dev.jna:jna:5.18.1@aar"

group = "org.zingolabs"

val zingolibRoot: File = rootDir.resolve("../..")
val builderOutput: Provider<Directory> = layout.buildDirectory.dir("binding-layer")
val gitDescribe: Provider<String> = providers.environmentVariable("ZINGO_MOBILE_GIT_DESCRIBE")
    .orElse(providers.gradleProperty("zingoMobileGitDescribe"))
val selectedAbi: Provider<String> = providers.gradleProperty("bindingLayerAbi")

val buildBindingLayer by tasks.registering(Exec::class) {
    description = "Builds the Binding Layer's Android libraries and Kotlin sources."
    workingDir = zingolibRoot
    environment("ZINGO_MOBILE_GIT_DESCRIBE", gitDescribe.getOrElse(""))
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
    inputs.property("gitDescribe", gitDescribe.getOrElse(""))
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
            java.srcDir(builderOutput.map { it.dir("kotlin") })
            jniLibs.srcDir(builderOutput.map { it.dir("jniLibs") })
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
}
