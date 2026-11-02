plugins {
    id("com.android.application") version "8.10.1" apply false
    kotlin("android") version "2.1.21" apply false
    kotlin("jvm") version "2.1.21" apply false
    kotlin("plugin.serialization") version "2.1.21" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.1.21" apply false
}

val kotlinFormatter by configurations.creating

dependencies {
    kotlinFormatter("com.facebook:ktfmt:0.54:jar-with-dependencies") { isTransitive = false }
}

val kotlinSources =
    fileTree(rootDir) {
        include("**/*.kt", "**/*.kts")
        exclude("**/build/**", ".gradle/**", ".kotlin/**")
    }

fun registerFormatter(name: String, check: Boolean) =
    tasks.register<JavaExec>(name) {
        classpath = kotlinFormatter
        mainClass.set("com.facebook.ktfmt.cli.Main")
        args("--kotlinlang-style")
        if (check) args("--dry-run", "--set-exit-if-changed")
        args(kotlinSources.files.sortedBy { it.path }.map { it.absolutePath })
    }

registerFormatter("formatKotlin", false)

registerFormatter("checkKotlinFormat", true)
