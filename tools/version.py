#!/usr/bin/env python3
"""Read VERSION.md, check Cargo.toml, and compare semver for releases.

    python3 tools/version.py read
    python3 tools/version.py check
    python3 tools/version.py set 0.1.2       # write VERSION.md, Cargo.toml, the lockfile, and the README badge
    python3 tools/version.py compare CURRENT PREVIOUS
    python3 tools/version.py latest          # newest v* name on stdin
    python3 tools/version.py test

compare prints "release" when CURRENT is strictly greater than PREVIOUS.
An empty previous version means no release exists yet, which is a release.
Equal or lower prints "skip". Both results exit 0. A bad version exits 1.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SEMVER = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z.-]+)?$"
)


def fail(message: str) -> None:
    raise SystemExit(message)


def read_version(path: Path | None = None) -> str:
    text = (path or ROOT / "VERSION.md").read_text(encoding="utf-8")
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if not SEMVER.match(stripped):
            fail(f"VERSION.md version is not semver: {stripped}")
        return stripped
    fail("VERSION.md has no version")
    return ""


def parse(version: str) -> tuple[int, int, int, tuple[str, ...]]:
    match = SEMVER.match(version.strip())
    if not match:
        fail(f"not semver: {version}")
    pre = match.group(4)
    return (
        int(match.group(1)),
        int(match.group(2)),
        int(match.group(3)),
        tuple(pre.split(".")) if pre else (),
    )


def cmp_id(left: str, right: str) -> int:
    left_num = left.isdigit()
    right_num = right.isdigit()
    if left_num and right_num:
        return (int(left) > int(right)) - (int(left) < int(right))
    if left_num != right_num:
        return -1 if left_num else 1
    return (left > right) - (left < right)


def cmp_ver(left: str, right: str) -> int:
    major_l, minor_l, patch_l, pre_l = parse(left)
    major_r, minor_r, patch_r, pre_r = parse(right)
    core_l = (major_l, minor_l, patch_l)
    core_r = (major_r, minor_r, patch_r)
    if core_l != core_r:
        return (core_l > core_r) - (core_l < core_r)
    if not pre_l and not pre_r:
        return 0
    if not pre_l:
        return 1
    if not pre_r:
        return -1
    for one, two in zip(pre_l, pre_r):
        order = cmp_id(one, two)
        if order:
            return order
    return (len(pre_l) > len(pre_r)) - (len(pre_l) < len(pre_r))


def cargo_version() -> str:
    data = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    try:
        return data["workspace"]["package"]["version"]
    except KeyError:
        fail("Cargo.toml has no workspace.package.version")
        return ""


def replace_workspace_version(text: str, version: str) -> str:
    lines = text.splitlines(keepends=True)
    in_package = False
    replaced = False
    out: list[str] = []
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_package = stripped == "[workspace.package]"
        elif in_package and not replaced and stripped.startswith("version"):
            newline = "\n" if line.endswith("\n") else ""
            line = f'version = "{version}"{newline}'
            replaced = True
        out.append(line)
    if not replaced:
        fail("Cargo.toml has no workspace.package.version")
    return "".join(out)


def write_version_line(text: str, version: str) -> str:
    lines = text.splitlines(keepends=True)
    replaced = False
    out: list[str] = []
    for line in lines:
        stripped = line.strip()
        if not replaced and stripped and not stripped.startswith("#"):
            newline = "\n" if line.endswith("\n") else ""
            line = f"{version}{newline}"
            replaced = True
        out.append(line)
    if not replaced:
        fail("VERSION.md has no version")
    return "".join(out)


def sync_readme(text: str, version: str) -> str:
    text = re.sub(
        r"(https://img\.shields\.io/badge/version-)[^-]+-",
        rf"\g<1>{version}-",
        text,
        count=1,
    )
    text = re.sub(r"(\[!\[Version )[^]]+(\])", rf"\g<1>{version}\2", text, count=1)
    text = re.sub(
        r"(Right now that line is `)[^`]+(`)",
        rf"\g<1>{version}\2",
        text,
        count=1,
    )
    return text


def set_version(version: str, root: Path | None = None, lock: bool = True) -> None:
    if not SEMVER.match(version):
        fail(f"not semver: {version}")
    root = root or ROOT
    version_path = root / "VERSION.md"
    cargo_path = root / "Cargo.toml"
    version_path.write_text(write_version_line(version_path.read_text(encoding="utf-8"), version), encoding="utf-8")
    cargo_path.write_text(replace_workspace_version(cargo_path.read_text(encoding="utf-8"), version), encoding="utf-8")
    readme_path = root / "README.md"
    if readme_path.is_file():
        readme_path.write_text(sync_readme(readme_path.read_text(encoding="utf-8"), version), encoding="utf-8")
    if lock and (root / "Cargo.lock").is_file():
        sync_lock_versions(root, version)
    if read_version(version_path) != version or cargo_version_at(cargo_path) != version:
        fail(f"failed to set version {version}")


def workspace_package_names(root: Path) -> set[str]:
    data = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    names: set[str] = set()
    for member in data.get("workspace", {}).get("members", []):
        manifest = root / member / "Cargo.toml"
        if not manifest.is_file():
            continue
        package = tomllib.loads(manifest.read_text(encoding="utf-8")).get("package", {})
        name = package.get("name")
        if isinstance(name, str):
            names.add(name)
    return names


def sync_lock_versions(root: Path, version: str) -> None:
    """Rewrite workspace package versions in Cargo.lock. Path deps do not pin a version."""
    path = root / "Cargo.lock"
    names = workspace_package_names(root)
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    out: list[str] = []
    index = 0
    while index < len(lines):
        line = lines[index]
        out.append(line)
        if line.startswith("name = "):
            name = line.split("=", 1)[1].strip().strip('"')
            nxt = index + 1
            if name in names and nxt < len(lines) and lines[nxt].startswith("version = "):
                newline = "\n" if lines[nxt].endswith("\n") else ""
                out.append(f'version = "{version}"{newline}')
                index = nxt
        index += 1
    path.write_text("".join(out), encoding="utf-8")


def cargo_version_at(path: Path) -> str:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    try:
        return data["workspace"]["package"]["version"]
    except KeyError:
        fail(f"{path} has no workspace.package.version")
        return ""


def check() -> str:
    want = read_version()
    got = cargo_version()
    if got != want:
        fail(f"VERSION.md is {want} but Cargo.toml workspace version is {got}")
    for path in ROOT.glob("*/*/Cargo.toml"):
        if "third_party" in path.parts or "target" in path.parts:
            continue
        package = tomllib.loads(path.read_text(encoding="utf-8")).get("package", {})
        member = package.get("version")
        if isinstance(member, str) and member != want:
            fail(f"{path.relative_to(ROOT)} version is {member}, VERSION.md is {want}")
    return want


def tag_version(name: str) -> str | None:
    text = name.strip()
    if not text or text.endswith("^{}"):
        return None
    text = text.split("/")[-1]
    if text.startswith("refs/tags/"):
        text = text[len("refs/tags/") :]
    if text.startswith("v"):
        text = text[1:]
    if SEMVER.match(text):
        return text
    return None


def newest(lines: list[str]) -> str:
    best = ""
    for line in lines:
        version = tag_version(line)
        if version and (not best or cmp_ver(version, best) > 0):
            best = version
    return best


def decision(current: str, previous: str) -> str:
    if not SEMVER.match(current):
        fail(f"not semver: {current}")
    previous = previous.strip()
    if previous in ("", "none", "null"):
        return "release"
    if not SEMVER.match(previous):
        fail(f"not semver: {previous}")
    return "release" if cmp_ver(current, previous) > 0 else "skip"


def run_tests() -> None:
    cases = [
        ("0.2.0", "0.1.0", "release"),
        ("0.1.0", "0.1.0", "skip"),
        ("0.1.0", "0.2.0", "skip"),
        ("0.1.0", "", "release"),
        ("0.1.0", "none", "release"),
        ("1.0.0", "0.9.9", "release"),
        ("0.1.10", "0.1.9", "release"),
        ("0.2.0", "0.2.0-rc.1", "release"),
        ("0.2.0-rc.1", "0.2.0", "skip"),
        ("0.2.0-rc.2", "0.2.0-rc.1", "release"),
    ]
    for current, previous, want in cases:
        got = decision(current, previous)
        label = previous or "none"
        if got != want:
            fail(f"compare {current} {label}: got {got}, want {want}")
        print(f"compare {current} {label} -> {got}")
    found = newest(["refs/tags/v0.1.0", "refs/tags/v0.1.5", "v0.2.0", "v0.2.0^{}", ""])
    if found != "0.2.0":
        fail(f"latest tags: got {found}, want 0.2.0")
    print(f"latest v0.1.0 v0.1.5 v0.2.0 -> {found}")
    if newest([]) != "":
        fail("latest of no tags should be empty")
    print("latest <no tags> -> <none>")
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "VERSION.md").write_text("0.1.0\n\n# note\n", encoding="utf-8")
        (root / "Cargo.toml").write_text(
            '[workspace.package]\nversion = "0.1.0"\nedition = "2024"\n',
            encoding="utf-8",
        )
        (root / "README.md").write_text(
            "[![Version 0.1.0](https://img.shields.io/badge/version-0.1.0-2ea44f)](VERSION.md)\n"
            "Right now that line is `0.1.0`.\n",
            encoding="utf-8",
        )
        set_version("0.1.2", root, lock=False)
        if read_version(root / "VERSION.md") != "0.1.2":
            fail("set did not write VERSION.md")
        if cargo_version_at(root / "Cargo.toml") != "0.1.2":
            fail("set did not write Cargo.toml")
        readme = (root / "README.md").read_text(encoding="utf-8")
        if "version-0.1.2-" not in readme or "Version 0.1.2" not in readme or "`0.1.2`" not in readme:
            fail(f"set did not update the README badge: {readme}")
        if "0.1.0" in readme:
            fail("set left 0.1.0 in the README")
    print("set 0.1.2 on a temp tree")
    matched = check()
    print(f"check VERSION.md == Cargo.toml ({matched})")
    print("ok")


def main(argv: list[str]) -> None:
    if len(argv) < 2:
        fail("usage: tools/version.py read|check|set|compare|latest|test")
    command = argv[1]
    if command == "read":
        print(read_version())
    elif command == "check":
        print(f"ok {check()}")
    elif command == "set":
        if len(argv) != 3:
            fail("usage: tools/version.py set VERSION")
        set_version(argv[2])
        print(f"set {argv[2]}")
    elif command == "compare":
        if len(argv) < 3:
            fail("usage: tools/version.py compare CURRENT [PREVIOUS]")
        previous = argv[3] if len(argv) > 3 else ""
        print(decision(argv[2], previous))
    elif command == "latest":
        lines = argv[2:] if len(argv) > 2 else sys.stdin.read().splitlines()
        print(newest(lines))
    elif command == "test":
        run_tests()
    else:
        fail(f"unknown command: {command}")


if __name__ == "__main__":
    main(sys.argv)
