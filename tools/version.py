#!/usr/bin/env python3
"""VERSION.md is the only version to edit. Everything else is copied from it.

    python3 tools/version.py read
    python3 tools/version.py sync            # Cargo.toml, Cargo.lock, README, Android, iOS
    python3 tools/version.py check           # report drift; never fails a release
    python3 tools/version.py set 0.1.2       # write VERSION.md, then sync
    python3 tools/version.py compare CURRENT PREVIOUS
    python3 tools/version.py latest          # newest v* name on stdin
    python3 tools/version.py test

compare prints "release" when CURRENT is strictly greater than PREVIOUS.
An empty previous version means no release exists yet, which is a release.
Equal or lower prints "skip". Both results exit 0. A bad version exits 1.
check exits 0 even when Cargo.toml still has an older version.
"""

from __future__ import annotations

import re
import subprocess
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


def version_code(version: str) -> int:
    """Android versionCode and iOS CFBundleVersion. Prerelease text is ignored."""
    major, minor, patch, _pre = parse(version)
    return major * 1_000_000 + minor * 1_000 + patch


def cargo_version() -> str:
    return cargo_version_at(ROOT / "Cargo.toml")


def replace_workspace_version(text: str, version: str) -> str:
    try:
        return replace_section_version(text, "[workspace.package]", version)
    except SystemExit:
        fail("Cargo.toml has no workspace.package.version")
        return ""


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
    """Point every version badge at VERSION.md's line.

    The badge is an HTML image, because a Markdown image inside a centered
    HTML block is shown as raw text on GitHub. The older Markdown form is
    still rewritten if it is present.
    """
    text = re.sub(
        r"(https://img\.shields\.io/badge/version-)[^-]+-",
        rf"\g<1>{version}-",
        text,
    )
    text = re.sub(r"(\[!\[Version )[^]]+(\])", rf"\g<1>{version}\2", text)
    text = re.sub(r'(alt="Version )[^"]+(")', rf"\g<1>{version}\2", text)
    text = re.sub(
        r"(Right now that line is `)[^`]+(`)",
        rf"\g<1>{version}\2",
        text,
        count=1,
    )
    return text


def replace_section_version(text: str, section: str, version: str) -> str:
    lines = text.splitlines(keepends=True)
    in_section = False
    replaced = False
    out: list[str] = []
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_section = stripped == section
        elif in_section and not replaced and stripped.startswith("version"):
            newline = "\n" if line.endswith("\n") else ""
            line = f'version = "{version}"{newline}'
            replaced = True
        out.append(line)
    if not replaced:
        fail(f"manifest has no {section} version")
    return "".join(out)


def set_version(version: str, root: Path | None = None, lock: bool = True) -> None:
    if not SEMVER.match(version):
        fail(f"not semver: {version}")
    root = root or ROOT
    version_path = root / "VERSION.md"
    version_path.write_text(write_version_line(version_path.read_text(encoding="utf-8"), version), encoding="utf-8")
    sync(root, lock=lock)
    if read_version(version_path) != version or cargo_version_at(root / "Cargo.toml") != version:
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


def render_lock(text: str, names: set[str], version: str) -> str:
    """Rewrite workspace package versions in Cargo.lock. Path deps do not pin a version."""
    lines = text.splitlines(keepends=True)
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
    return "".join(out)


def sync_lock_versions(root: Path, version: str) -> None:
    path = root / "Cargo.lock"
    names = workspace_package_names(root)
    path.write_text(render_lock(path.read_text(encoding="utf-8"), names, version), encoding="utf-8")


SKIP_DIRS = {"third_party", "target", ".git"}
MOBILE_DIRS = {"mobile", "android", "ios"}


def mobile_files(root: Path) -> list[Path]:
    found: list[Path] = []
    if not root.is_dir():
        return found
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        parts = path.relative_to(root).parts
        if any(part in SKIP_DIRS for part in parts):
            continue
        if not any(part in MOBILE_DIRS for part in parts):
            continue
        name = path.name
        if name in ("build.gradle", "build.gradle.kts", "Info.plist") or name.endswith(".pbxproj"):
            found.append(path)
    return found


def apply_mobile(text: str, path: Path, version: str) -> str:
    code = str(version_code(version))
    name = path.name
    if name in ("build.gradle", "build.gradle.kts"):
        text = re.sub(r'(versionName\s*=\s*")[^"]*(")', rf"\g<1>{version}\2", text)
        text = re.sub(r'(versionName\s+")[^"]*(")', rf"\g<1>{version}\2", text)
        text = re.sub(r"(versionCode\s*=\s*)\d+", rf"\g<1>{code}", text)
        text = re.sub(r"(versionCode\s+)\d+", rf"\g<1>{code}", text)
        return text
    if name == "Info.plist":
        text = re.sub(
            r"(<key>CFBundleShortVersionString</key>\s*<string>)[^<]*(</string>)",
            rf"\g<1>{version}\2",
            text,
        )
        text = re.sub(
            r"(<key>CFBundleVersion</key>\s*<string>)[^<]*(</string>)",
            rf"\g<1>{code}\2",
            text,
        )
        return text
    text = re.sub(r"(MARKETING_VERSION = )[^;]*;", rf"\g<1>{version};", text)
    text = re.sub(r"(CURRENT_PROJECT_VERSION = )[^;]*;", rf"\g<1>{code};", text)
    return text


def write_if_changed(path: Path, text: str, changed: list[str], root: Path) -> None:
    current = path.read_text(encoding="utf-8") if path.is_file() else None
    if current != text:
        path.write_text(text, encoding="utf-8")
        changed.append(str(path.relative_to(root)))


def sync(root: Path | None = None, lock: bool = True) -> list[str]:
    """Copy VERSION.md into Cargo, the README badge, and mobile manifests."""
    root = root or ROOT
    version = read_version(root / "VERSION.md")
    changed: list[str] = []
    cargo_path = root / "Cargo.toml"
    write_if_changed(
        cargo_path,
        replace_workspace_version(cargo_path.read_text(encoding="utf-8"), version),
        changed,
        root,
    )
    for manifest in sorted(root.glob("*/*/Cargo.toml")):
        if any(part in SKIP_DIRS for part in manifest.relative_to(root).parts):
            continue
        package = tomllib.loads(manifest.read_text(encoding="utf-8")).get("package", {})
        if isinstance(package.get("version"), str):
            write_if_changed(
                manifest,
                replace_section_version(manifest.read_text(encoding="utf-8"), "[package]", version),
                changed,
                root,
            )
    if lock and (root / "Cargo.lock").is_file():
        lock_path = root / "Cargo.lock"
        rendered = render_lock(
            lock_path.read_text(encoding="utf-8"),
            workspace_package_names(root),
            version,
        )
        write_if_changed(lock_path, rendered, changed, root)
    readme_path = root / "README.md"
    if readme_path.is_file():
        write_if_changed(
            readme_path,
            sync_readme(readme_path.read_text(encoding="utf-8"), version),
            changed,
            root,
        )
    for path in mobile_files(root):
        write_if_changed(path, apply_mobile(path.read_text(encoding="utf-8"), path, version), changed, root)
    return changed


def drift(root: Path | None = None) -> list[str]:
    """Paths sync would rewrite. Does not write, and does not fail."""
    root = root or ROOT
    version = read_version(root / "VERSION.md")
    stale: list[str] = []

    def consider(path: Path, rendered: str) -> None:
        if path.is_file() and path.read_text(encoding="utf-8") != rendered:
            stale.append(str(path.relative_to(root)))

    cargo_path = root / "Cargo.toml"
    consider(cargo_path, replace_workspace_version(cargo_path.read_text(encoding="utf-8"), version))
    if (root / "Cargo.lock").is_file():
        lock_path = root / "Cargo.lock"
        consider(
            lock_path,
            render_lock(lock_path.read_text(encoding="utf-8"), workspace_package_names(root), version),
        )
    for path in mobile_files(root):
        consider(path, apply_mobile(path.read_text(encoding="utf-8"), path, version))
    return stale


def cargo_version_at(path: Path) -> str:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    try:
        return data["workspace"]["package"]["version"]
    except KeyError:
        fail(f"{path} has no workspace.package.version")
        return ""


def check(root: Path | None = None) -> str:
    """Read VERSION.md. A stale Cargo.toml is drift, not an error."""
    root = root or ROOT
    return read_version(root / "VERSION.md")


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
            '<p align="center">\n'
            '  <a href="VERSION.md"><img alt="Version 0.1.0" src="https://img.shields.io/badge/version-0.1.0-2ea44f"></a>\n'
            "</p>\n"
            "\n"
            "[![Version 0.1.0](https://img.shields.io/badge/version-0.1.0-2ea44f)](VERSION.md)\n"
            "Right now that line is `0.1.0`.\n",
            encoding="utf-8",
        )
        (root / "Cargo.lock").write_text(
            '[[package]]\nname = "demo"\nversion = "0.1.0"\n\n[[package]]\nname = "other"\nversion = "0.1.0"\n',
            encoding="utf-8",
        )
        (root / "crates" / "demo").mkdir(parents=True)
        (root / "crates" / "demo" / "Cargo.toml").write_text(
            '[package]\nname = "demo"\nversion = "0.9.9"\n',
            encoding="utf-8",
        )
        cargo = (root / "Cargo.toml").read_text(encoding="utf-8")
        (root / "Cargo.toml").write_text(
            cargo + '\n[workspace]\nmembers = ["crates/demo"]\n',
            encoding="utf-8",
        )
        gradle = root / "mobile" / "android" / "app" / "build.gradle.kts"
        gradle.parent.mkdir(parents=True)
        gradle.write_text(
            'android {\n    defaultConfig {\n        versionCode = 1\n        versionName = "0.0.1"\n    }\n}\n',
            encoding="utf-8",
        )
        plist = root / "mobile" / "ios" / "Info.plist"
        plist.parent.mkdir(parents=True)
        plist.write_text(
            "<key>CFBundleShortVersionString</key>\n<string>0.0.1</string>\n"
            "<key>CFBundleVersion</key>\n<string>1</string>\n",
            encoding="utf-8",
        )
        project = root / "mobile" / "ios" / "App.xcodeproj" / "project.pbxproj"
        project.parent.mkdir(parents=True)
        project.write_text(
            "MARKETING_VERSION = 0.0.1;\nCURRENT_PROJECT_VERSION = 1;\n",
            encoding="utf-8",
        )
        if not drift(root):
            fail("drift was empty on a stale tree")
        if check(root) != "0.1.0":
            fail("check rejected a stale tree")
        set_version("0.1.2", root)
        if read_version(root / "VERSION.md") != "0.1.2":
            fail("set did not write VERSION.md")
        if cargo_version_at(root / "Cargo.toml") != "0.1.2":
            fail("set did not write Cargo.toml")
        readme = (root / "README.md").read_text(encoding="utf-8")
        if "version-0.1.2-" not in readme or "Version 0.1.2" not in readme or "`0.1.2`" not in readme:
            fail(f"set did not update the README badge: {readme}")
        if "0.1.0" in readme:
            fail("set left 0.1.0 in the README")
        member = (root / "crates" / "demo" / "Cargo.toml").read_text(encoding="utf-8")
        if 'version = "0.1.2"' not in member:
            fail(f"set did not update the member manifest: {member}")
        lock = (root / "Cargo.lock").read_text(encoding="utf-8")
        if 'name = "demo"\nversion = "0.1.2"' not in lock:
            fail(f"set did not update the lockfile member: {lock}")
        if 'name = "other"\nversion = "0.1.0"' not in lock:
            fail("set rewrote a crate that is not a workspace member")
        gradle_text = gradle.read_text(encoding="utf-8")
        if 'versionName = "0.1.2"' not in gradle_text or "versionCode = 1002" not in gradle_text:
            fail(f"set did not update Android: {gradle_text}")
        plist_text = plist.read_text(encoding="utf-8")
        if "<string>0.1.2</string>" not in plist_text or "<string>1002</string>" not in plist_text:
            fail(f"set did not update Info.plist: {plist_text}")
        project_text = project.read_text(encoding="utf-8")
        if "MARKETING_VERSION = 0.1.2;" not in project_text or "CURRENT_PROJECT_VERSION = 1002;" not in project_text:
            fail(f"set did not update the Xcode project: {project_text}")
        if version_code("1.2.3-rc.1") != 1_002_003:
            fail("version code kept a prerelease")
        if drift(root):
            fail(f"drift remained after sync: {drift(root)}")
        if sync(root):
            fail("sync rewrote files that were already current")
    print("set 0.1.2 on a temp tree")
    matched = check()
    print(f"check VERSION.md ({matched}); drift does not fail")
    print("ok")


def main(argv: list[str]) -> None:
    # The Servo tree is patched before any cargo command CI runs through here.
    script = ROOT / "tools" / "patch-servo-processes.py"
    if script.is_file():
        subprocess.run([sys.executable, str(script)], check=True)
    if len(argv) < 2:
        fail("usage: tools/version.py read|sync|check|set|compare|latest|test")
    command = argv[1]
    if command == "read":
        print(read_version())
    elif command == "sync":
        changed = sync()
        if changed:
            print("synced " + ", ".join(changed))
        else:
            print(f"synced {read_version()}")
    elif command == "check":
        version = check()
        stale = drift()
        if stale:
            print(f"VERSION.md is {version}; these files differ and will be patched at build time:")
            for path in stale:
                print(f"  {path}")
        else:
            print(f"ok {version}")
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
