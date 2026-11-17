plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val repoRoot = rootProject.projectDir.parentFile.parentFile
val generatedAssets = layout.buildDirectory.dir("generated/mobile-assets")

val installMobileAssets = tasks.register<Exec>("installMobileAssets") {
    workingDir = repoRoot
    val dest = generatedAssets.map { it.asFile.absolutePath }
    commandLine("sh", repoRoot.resolve("tools/install-mobile-assets.sh").absolutePath, dest.get())
    inputs.file(repoRoot.resolve("VERSION.md"))
    inputs.file(repoRoot.resolve("tools/install-mobile-assets.sh"))
    inputs.dir(repoRoot.resolve("assets/fonts/roboto"))
    inputs.file(repoRoot.resolve("assets/brand/png/logo-512.png"))
    inputs.file(repoRoot.resolve("assets/brand/png/mark-64.png"))
    outputs.dir(generatedAssets)
}

android {
    namespace = "org.mithu.lite"
    compileSdk = 35

    defaultConfig {
        applicationId = "org.mithu.lite"
        minSdk = 26
        targetSdk = 35
        versionCode = 1009
        versionName = "0.1.9"
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
    sourceSets.named("main") {
        assets.srcDir(generatedAssets)
    }
}

tasks.named("preBuild").configure {
    dependsOn(installMobileAssets)
}

dependencies {
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.webkit:webkit:1.12.1")
}
