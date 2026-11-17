import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}
val localSigning = Properties().apply {
    rootProject.file("local.properties").takeIf { it.isFile }?.inputStream()?.use { load(it) }
}
fun signingValue(environment: String, property: String): String? =
    providers.environmentVariable(environment).orNull?.takeIf { it.isNotBlank() }
        ?: localSigning.getProperty(property)?.takeIf { it.isNotBlank() }
val releaseStore = signingValue("SPECTRUM_RELEASE_STORE_FILE", "spectrum.release.storeFile")
val releaseStorePassword = signingValue("SPECTRUM_RELEASE_STORE_PASSWORD", "spectrum.release.storePassword")
val releaseAlias = signingValue("SPECTRUM_RELEASE_KEY_ALIAS", "spectrum.release.keyAlias")
val releaseKeyPassword = signingValue("SPECTRUM_RELEASE_KEY_PASSWORD", "spectrum.release.keyPassword")
val signingValues = listOf(releaseStore, releaseStorePassword, releaseAlias, releaseKeyPassword)
check(signingValues.all { it == null } || signingValues.all { it != null }) { "Provide all four Spectrum release signing values, or none for an unsigned demo APK" }
android {
    namespace = "io.github.lawchli.spectrum"
    compileSdk = 36
    buildToolsVersion = "36.0.0"
    defaultConfig {
        applicationId = "io.github.lawchli.spectrum"
        minSdk = 26
        targetSdk = 36
        versionCode = 30001
        versionName = "0.3.0-beta.1"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    signingConfigs {
        if (releaseStore != null) create("release") {
            val keyStore = rootProject.file(releaseStore).canonicalFile
            check(!keyStore.toPath().startsWith(rootProject.projectDir.parentFile.canonicalFile.toPath())) { "Release keystore must be outside the repository" }
            storeFile = keyStore
            storePassword = releaseStorePassword
            keyAlias = releaseAlias
            keyPassword = releaseKeyPassword
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            if (releaseStore != null) signingConfig = signingConfigs.getByName("release")
        }
    }
    buildFeatures { compose = true }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
}
dependencies {
    implementation(project(":spectrum"))
    implementation(platform("androidx.compose:compose-bom:2025.04.01"))
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.compose.material3:material3")
    testImplementation("junit:junit:4.13.2")
}
