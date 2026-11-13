#!/usr/bin/env python3
"""Package built Windows/Linux binaries without modifying the installation."""
import argparse, hashlib, json, shutil, subprocess, tarfile, tomllib, zipfile
from pathlib import Path

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
