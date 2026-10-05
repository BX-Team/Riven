/// The OS name Mojang's rules and native classifiers use.
pub const NAME: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_os = "macos") {
    "osx"
} else {
    "linux"
};

/// What native classifiers call the CPU: `x86`, `arm64`, or empty for x86-64.
pub fn native_arch(x64_java: bool) -> &'static str {
    if x64_java || cfg!(target_arch = "x86_64") {
        ""
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        "arm32"
    }
}

/// `${arch}` in old native classifiers: the JVM's pointer width.
pub const BITS: &str = if cfg!(target_pointer_width = "64") {
    "64"
} else {
    "32"
};

pub const CLASSPATH_SEPARATOR: &str = if cfg!(windows) { ";" } else { ":" };

pub fn java_exe() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}
