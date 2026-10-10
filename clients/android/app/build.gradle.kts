plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}
android {
    namespace = "org.sarmg.xcoc"
    compileSdk = 36
    defaultConfig {
        applicationId = "org.sarmg.xcoc"
        minSdk = 26
        targetSdk = 36
        versionCode = 1_001_000
        versionName = "1.1.0"
        ndk { abiFilters += "arm64-v8a" }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    sourceSets["main"].jniLibs.srcDir("src/main/jniLibs")
    testOptions.unitTests.all {
        it.jvmArgs("-Djava.library.path=${file("../../../target/debug").absolutePath}")
    }
}
dependencies { testImplementation("junit:junit:4.13.2") }
