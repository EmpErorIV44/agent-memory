"""Package a native Cargo release build with documentation and a SHA-256 sidecar."""

import argparse
import gzip
import hashlib
import io
from pathlib import Path
import re
import tarfile


TARGETS = {"x86_64-unknown-linux-gnu", "aarch64-apple-darwin"}


def package(root, target, version, output):
    if target not in TARGETS:
        raise ValueError(f"Unsupported release target: {target}")
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version):
        raise ValueError("Invalid release version")
    root, output = Path(root), Path(output)
    files = {
        "agent-memory": root / "target" / target / "release" / "agent-memory",
        "README.md": root / "README.md",
        "LICENSE": root / "LICENSE",
    }
    for source in files.values():
        if source.is_symlink() or not source.is_file():
            raise ValueError(f"Missing or invalid release input: {source}")
    output.mkdir(parents=True, exist_ok=True)
    name = f"agent-memory-{version}-{target}"
    archive = output / f"{name}.tar.gz"
    # Normalize archive metadata; package checksums do not depend on file mtimes.
    with archive.open("wb") as raw:
        with gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as tar:
                for filename, source in files.items():
                    data = source.read_bytes()
                    entry = tarfile.TarInfo(f"{name}/{filename}")
                    entry.size = len(data)
                    entry.mode = 0o755 if filename == "agent-memory" else 0o644
                    tar.addfile(entry, io.BytesIO(data))
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(
        f"{checksum}  {archive.name}\n", encoding="ascii"
    )
    return archive


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=sorted(TARGETS))
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    print(package(root, args.target, args.version, args.output))
