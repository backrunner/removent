use std::env;
use std::process::Command;

fn detect_sdk_major_version() -> Option<u32> {
    let output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-version"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version_str = String::from_utf8_lossy(&output.stdout);
    let major = version_str.trim().split('.').next()?;
    major.parse().ok()
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DOCS_RS");
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    println!("cargo:rerun-if-env-changed=SDKROOT");

    if env::var("DOCS_RS").is_ok() {
        return;
    }

    let _ = detect_sdk_major_version(); // currently unused; reserved for future macos_* feature flags

    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=IOSurface");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=CoreMedia");
    println!("cargo:rustc-link-lib=framework=CoreVideo");
    println!("cargo:rustc-link-lib=framework=Metal");

    let swift_dir = "swift-bridge";
    let out_dir = env::var("OUT_DIR").unwrap();
    let swift_build_dir = format!("{out_dir}/swift-build");

    println!("cargo:rerun-if-changed={swift_dir}");

    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => panic!("unsupported architecture: {other}"),
    };
    let ios = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios");
    let simulator = env::var("TARGET").unwrap().ends_with("-sim") || arch == "x86_64";
    let triple = if ios {
        format!("{arch}-apple-ios17.0{}", if simulator { "-simulator" } else { "" })
    } else { format!("{arch}-apple-macosx13.0") };
    let sdk = if ios { if simulator { "iphonesimulator" } else { "iphoneos" } } else { "macosx" };
    let sdk_path = Command::new("xcrun").args(["--sdk", sdk, "--show-sdk-path"]).output().unwrap();
    assert!(sdk_path.status.success(), "Apple SDK not found");
    let sdk_path = String::from_utf8(sdk_path.stdout).unwrap().trim().to_owned();
    let swift_args = vec!["build", "-c", "release", "--triple", &triple,
        "--sdk", &sdk_path, "--package-path", swift_dir, "--scratch-path", &swift_build_dir];

    let output = Command::new("swift")
        .env_remove("SDKROOT")
        .args(&swift_args)
        .output()
        .expect("Failed to build Swift bridge");

    if !output.status.success() {
        eprintln!(
            "Swift build STDOUT:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        eprintln!(
            "Swift build STDERR:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        panic!(
            "Swift build failed with exit code: {:?}",
            output.status.code()
        );
    }

    link_swift_bridge(&swift_build_dir);
}

fn link_swift_bridge(swift_build_dir: &str) {
    println!("cargo:rustc-link-search=native={swift_build_dir}/release");
    println!("cargo:rustc-link-lib=static=AppleCFBridge");

    println!("cargo:rustc-link-lib=framework=Foundation");

    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    if let Ok(output) = Command::new("xcode-select").arg("-p").output() {
        if output.status.success() {
            let xcode_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let swift_lib_path =
                format!("{xcode_path}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{swift_lib_path}");
        }
    }
}
