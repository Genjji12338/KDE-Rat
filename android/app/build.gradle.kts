import java.util.Properties

// Release signing: a properties file outside the repository (never committed):
// storeFile, storePassword, keyAlias, keyPassword. Without it the release APK
// is built unsigned (F-Droid signs its own builds).
val signing = Properties().apply {
    val f = file(System.getenv("TUNNELRAT_SIGNING") ?: "${System.getProperty("user.home")}/.android-keys/tunnelrat-release.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose.compiler)
}

android {
    namespace = "dev.tunnelrat"
    compileSdk = 37

    defaultConfig {
        applicationId = "dev.tunnelrat"
        minSdk = 29
        targetSdk = 37
        versionCode = 2
        versionName = "0.2.0"
    }

    signingConfigs {
        if (!signing.isEmpty) {
            create("release") {
                storeFile = file(signing.getProperty("storeFile"))
                storePassword = signing.getProperty("storePassword")
                keyAlias = signing.getProperty("keyAlias")
                keyPassword = signing.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("release")
        }
    }

    // No Google-encrypted dependency list in the APK (F-Droid rejects it).
    dependenciesInfo {
        includeInApk = false
        includeInBundle = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons)
    implementation(libs.okhttp)
    implementation(libs.camera.camera2)
    implementation(libs.camera.lifecycle)
    implementation(libs.camera.compose)
    implementation(libs.zxing.core)
    debugImplementation(libs.compose.ui.tooling)
    testImplementation(libs.junit)
}
