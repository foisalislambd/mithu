0.1.6

# The first non-comment line is the only version to edit (semver:
# MAJOR.MINOR.PATCH, with an optional -prerelease). Cargo.toml, Cargo.lock,
# the README badge, Android versionName/versionCode, and iOS
# CFBundleShortVersionString/CFBundleVersion are copied from this line.
# Refresh them with: python3 tools/version.py sync
# CI and the release workflow run that sync before compiling. A stale
# Cargo.toml does not block the release.
#
# To cut a release, change this line and push to main.
# The release workflow publishes v<version> only when this version is strictly
# greater than the newest v* tag. An equal or lower version skips with success.
