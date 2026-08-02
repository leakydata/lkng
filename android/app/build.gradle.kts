plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "org.lkng.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "org.lkng.app"
        minSdk = 26          // exec of a bundled binary from nativeLibraryDir
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    // Release signing, configured from a properties file that is NOT in the
    // repository.
    //
    // The keystore and its passwords stay outside version control and off
    // any build machine that does not need them. For an app in this
    // category that is not hygiene, it is the whole trust model: whoever
    // holds this key can ship an update that every existing install accepts
    // silently, to users for whom a malicious update is not an inconvenience.
    //
    // Create `android/keystore.properties` (git-ignored) with:
    //
    //     storeFile=/absolute/path/to/lkng-release.jks
    //     storePassword=...
    //     keyAlias=lkng
    //     keyPassword=...
    //
    // and generate the key with:
    //
    //     keytool -genkeypair -v -keystore lkng-release.jks \
    //       -keyalg RSA -keysize 4096 -validity 10000 -alias lkng
    //
    // 4096-bit and a 27-year validity because Android app signing keys
    // cannot be rotated for an existing listing without losing the ability
    // to update it. Back the file up somewhere durable and offline: losing
    // it means every existing install is stranded on its last version, with
    // no way to publish a security fix to them.
    val keystorePropsFile = rootProject.file("keystore.properties")
    val keystoreProps = java.util.Properties().apply {
        if (keystorePropsFile.exists()) {
            keystorePropsFile.inputStream().use { load(it) }
        }
    }

    signingConfigs {
        if (keystorePropsFile.exists()) {
            create("release") {
                storeFile = file(keystoreProps.getProperty("storeFile"))
                storePassword = keystoreProps.getProperty("storePassword")
                keyAlias = keystoreProps.getProperty("keyAlias")
                keyPassword = keystoreProps.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Signed only when the properties file is present. Deliberately
            // NOT falling back to the debug key: a release build signed with
            // the debug key is installable and looks fine, which is exactly
            // how it ends up distributed to someone.
            signingConfig = signingConfigs.findByName("release")
        }
        debug {
            isDebuggable = true
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    buildFeatures { buildConfig = true }

    // The Freenet node ships as a native library so Android extracts it
    // and marks it executable; a plain asset cannot be exec'd on modern
    // Android (W^X on app-private storage).
    packaging {
        jniLibs { useLegacyPackaging = true }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.webkit:webkit:1.12.1")
}
