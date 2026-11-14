#!/usr/bin/env python3
"""Package built Windows/Linux binaries without modifying the installation."""
import argparse, hashlib, json, os, shutil, subprocess, tarfile, tomllib, zipfile
from pathlib import Path

def windows_crt_directory():
    """Use redistributable files from the installed build tools, never System32."""
    configured = os.environ.get("VCToolsRedistDir")
    if configured:
        redist = Path(configured)
        candidates = list(redist.glob("x64/Microsoft.VC*.CRT"))
    else:
        vswhere = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
        installation = subprocess.check_output([
            str(vswhere), "-latest", "-products", "*", "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath",
        ], text=True).strip()
        if not installation:
            raise RuntimeError("Visual C++ build tools and their redistributable files are required")
        candidates = list((Path(installation) / "VC/Redist/MSVC").glob("*/x64/Microsoft.VC*.CRT"))
    candidates = [p for p in candidates if (p / "vcruntime140.dll").is_file()]
    if not candidates:
        raise RuntimeError("The x64 Visual C++ redistributable runtime was not found")
    def version(path):
        return tuple(int(part) for part in path.parent.parent.name.split(".") if part.isdigit())
    return max(candidates, key=version)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=["windows-x86_64", "linux-x86_64", "linux-aarch64"])
    parser.add_argument("--bin-dir", type=Path, default=Path("target/release"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    out = root / "dist" / ("Removent-" + args.target)
    if out.exists(): shutil.rmtree(out)
    out.mkdir(parents=True)
    suffix = ".exe" if args.target.startswith("windows") else ""
    for name in ("removent", "removentd", "removent-tray"):
        shutil.copy2(args.bin_dir / (name + suffix), out)
    guide = (root / "docs/guides/windows-linux.md").read_text()
    (out / "README.md").write_text(guide.replace("../validation/windows-linux-2026-11-13.md", "VALIDATION.md"))
    shutil.copy2(root / "docs/validation/windows-linux-2026-11-13.md", out / "VALIDATION.md")
    shutil.copy2(root / "LICENSE", out)
    revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, capture_output=True, text=True)
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    (out / "BUILD.json").write_text(json.dumps({
        "version": manifest["workspace"]["package"]["version"],
        "platform": args.target,
        "source_commit": revision.stdout.strip() if revision.returncode == 0 else None,
    }, indent=2) + "\n")
    # Carry explicitly built runtime DLLs if a Windows dependency provides them.
    if suffix:
        runtime = windows_crt_directory()
        for dll in runtime.glob("*.dll"): shutil.copy2(dll, out)
        (out / "WINDOWS-RUNTIME.txt").write_text(
            "Microsoft Visual C++ Runtime\n"
            "Copyright Microsoft Corporation. All rights reserved.\n\n"
            "These redistributable DLLs are Microsoft components, separate from\n"
            "Removent's Apache-2.0 license. They are supplied for application-local\n"
            "deployment under the Visual Studio redistribution terms.\n"
            "https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files\n"
            "https://visualstudio.microsoft.com/license-terms/\n"
        )
        for dll in args.bin_dir.glob("*.dll"): shutil.copy2(dll, out)
        archive = out.with_suffix(".zip")
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
            for file in sorted(out.iterdir()): z.write(file, out.name + "/" + file.name)
    else:
        archive = out.with_suffix(".tar.gz")
        with tarfile.open(archive,"w:gz") as t: t.add(out, arcname=out.name)
    archive.with_name(archive.name + ".sha256").write_text(hashlib.sha256(archive.read_bytes()).hexdigest()+"  "+archive.name+"\n")
    print(archive)
if __name__ == "__main__": main()
