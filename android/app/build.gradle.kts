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

    buildTypes {
        release {
            isMinifyEnabled = false
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
