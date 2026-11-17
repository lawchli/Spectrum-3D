plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}
val exportAssets by tasks.registering(Exec::class) {
    workingDir(rootProject.projectDir.parentFile)
    commandLine("node", "scripts/export-android.mjs")
    inputs.dir(rootProject.projectDir.parentFile.resolve("web"))
    inputs.files(rootProject.projectDir.parentFile.resolve("scripts/export-android.mjs"), rootProject.projectDir.parentFile.resolve("scripts/export-shared.mjs"))
    outputs.dir(layout.buildDirectory.dir("generated/spectrum/assets"))
}
android {
    namespace = "io.github.lawchli.spectrum.core"
    compileSdk = 36
    buildToolsVersion = "36.0.0"
    defaultConfig {
        minSdk = 26
        consumerProguardFiles("consumer-rules.pro")
    }
    buildTypes { release { isMinifyEnabled = false } }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
    sourceSets["main"].assets.srcDir(layout.buildDirectory.dir("generated/spectrum/assets"))
    sourceSets["test"].resources.srcDir("../../tests/fixtures")
    lint { targetSdk = 36 }
}
tasks.named("preBuild").configure { dependsOn(exportAssets) }
dependencies { testImplementation("junit:junit:4.13.2") }
