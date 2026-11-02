import java.util.Properties

plugins {
    id("com.android.application")
    kotlin("android")
    kotlin("plugin.serialization")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "sh.zuse.android"
    compileSdk = 35
    defaultConfig {
        applicationId = "sh.zuse.android"
        minSdk = 28
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    defaultConfig {
        val profiles =
            groovy.json.JsonSlurper().parse(rootProject.file("config/deployment-profiles.json"))
                as Map<*, *>
        val profileName = providers.gradleProperty("deploymentProfile").orElse("production").get()
        require(profileName in setOf("production", "staging")) { "Unknown deploymentProfile" }
        val profile = profiles[profileName] as Map<*, *>
        if (profileName == "staging") { applicationIdSuffix = ".staging"; versionNameSuffix = "-staging" }
        manifestPlaceholders["appLabel"] = if (profileName == "staging") "Zuse Staging" else "Zuse"
        buildConfigField("String", "DEPLOYMENT_PROFILE", "\"$profileName\"")
        buildConfigField("String", "API_URL", "\"${profile["apiUrl"]}\"")
        buildConfigField("String", "WORKOS_CLIENT_ID", "\"${profile["workosPublicClientId"]}\"")
    }
    testOptions { unitTests.isIncludeAndroidResources = true }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }
}

dependencies {
    testImplementation("org.robolectric:robolectric:4.15.1")
    testImplementation(platform("androidx.compose:compose-bom:2025.05.01"))
    testImplementation("androidx.compose.ui:ui-test-junit4")
    testImplementation("androidx.test:core:1.6.1")
    implementation(project(":core"))
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    androidTestImplementation("com.squareup.okhttp3:mockwebserver:4.12.0")
    implementation(platform("androidx.compose:compose-bom:2025.05.01"))
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.0")
    implementation("androidx.compose.material3.adaptive:adaptive:1.1.0")
    implementation("androidx.compose.material3.adaptive:adaptive-layout:1.1.0")
    implementation("androidx.compose.material3.adaptive:adaptive-navigation:1.1.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.8.1")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    implementation("io.noties.markwon:core:4.6.2")
    implementation("io.noties.markwon:inline-parser:4.6.2")
    implementation("io.noties.markwon:ext-latex:4.6.2")
    implementation("androidx.javascriptengine:javascriptengine:1.0.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-guava:1.10.2")
    implementation("io.noties.markwon:ext-tables:4.6.2")
    implementation("io.noties.markwon:ext-strikethrough:4.6.2")
    androidTestImplementation(platform("androidx.compose:compose-bom:2025.05.01"))
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}

tasks.withType<Test>().configureEach {
    maxHeapSize = "2g"
    systemProperty(
        "zuse.screenshots",
        layout.buildDirectory.dir("screenshots").get().asFile.absolutePath,
    )
}

// Reuse the same audited Noise channel on physical devices and the emulator.
val pairingLibraries = layout.buildDirectory.dir("generated/paired-jni")
val buildPairingNative by
    tasks.registering(Exec::class) {
        inputs.files(
            fileTree(rootProject.file("../../crates/mobile-bridge")),
            fileTree(rootProject.file("../../crates/remote")),
            rootProject.file("../../Cargo.toml"),
            rootProject.file("../../Cargo.lock"),
            rootProject.file("native/pairing-jni.c"),
            rootProject.file("scripts/build-pairing.py"),
        )
        outputs.dir(pairingLibraries)
        doFirst {
            val local = Properties()
            rootProject
                .file("local.properties")
                .takeIf { it.exists() }
                ?.inputStream()
                ?.use { local.load(it) }
            val sdk =
                System.getenv("ANDROID_HOME")
                    ?: System.getenv("ANDROID_SDK_ROOT")
                    ?: local.getProperty("sdk.dir")
                    ?: error("Set ANDROID_HOME or sdk.dir before building native pairing")
            val ndk = System.getenv("ANDROID_NDK_HOME") ?: "$sdk/ndk/26.3.11579264"
            commandLine(
                "python3",
                rootProject.file("scripts/build-pairing.py"),
                "--ndk",
                ndk,
                "--output",
                pairingLibraries.get().asFile,
            )
        }
    }

android.sourceSets["main"].jniLibs.srcDir(pairingLibraries)

tasks.named("preBuild").configure { dependsOn(buildPairingNative) }
