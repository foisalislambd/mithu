plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "org.mithu.lite"
    compileSdk = 35

    defaultConfig {
        applicationId = "org.mithu.lite"
        minSdk = 26
        targetSdk = 35
        versionCode = 1005
        versionName = "0.1.5"
    }

    flavorDimensions += "engine"
    productFlavors {
        create("lite") {
            dimension = "engine"
            applicationId = "org.mithu.lite"
            resValue("string", "app_name", "Mithu Lite")
        }
        create("servo") {
            dimension = "engine"
            applicationId = "org.mithu.servo"
            resValue("string", "app_name", "Mithu")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    implementation("androidx.appcompat:appcompat:1.7.0")
}
