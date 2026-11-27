plugins {
    java
    id("com.gradleup.shadow") version "9.6.1"
}

group = "org.bxteam.riven"

repositories {
    mavenCentral()
    maven("https://libraries.minecraft.net")
}

val authlibs = listOf(
    "1.5.25", "1.6.25", "2.1.28", "2.3.31", "3.2.38", "3.3.39", "3.5.41", "3.11.49", "3.11.50", "3.16.29",
    "3.18.38", "4.0.43", "5.0.47", "6.0.52", "6.0.54", "6.0.55", "6.0.57", "6.0.58", "7.0.61", "7.0.63",
    "9.0.75", "10.0.77",
)

dependencies {
    implementation("org.ow2.asm:asm:9.11.1")
    testImplementation(platform("org.junit:junit-bom:6.1.3"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.compileJava { options.release = 8; options.compilerArgs.add("-Xlint:-options") }
tasks.compileTestJava { options.release = 21 }

tasks.withType<AbstractArchiveTask>().configureEach {
    isPreserveFileTimestamps = false
    isReproducibleFileOrder = true
}

tasks.jar { enabled = false }

tasks.shadowJar {
    archiveFileName = "riven-agent.jar"
    // Riven embeds the jar; the build writes it next to the crate that does.
    destinationDirectory = layout.projectDirectory.dir("../crates/riven-launch")
    relocate("org.objectweb.asm", "org.bxteam.riven.agent.asm")
    minimize()
    manifest { attributes("Premain-Class" to "org.bxteam.riven.agent.Agent") }
}

tasks.assemble { dependsOn(tasks.shadowJar) }

tasks.test {
    useJUnitPlatform()
    val resolved = authlibs.associateWith { configurations.detachedConfiguration(dependencies.create("com.mojang:authlib:$it")) }
    resolved.values.forEach { inputs.files(it) }
    doFirst {
        systemProperty("authlibs", resolved.entries.joinToString(";") { (version, files) ->
            version + "=" + files.files.joinToString(File.pathSeparator)
        })
    }
}
