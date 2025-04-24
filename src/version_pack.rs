use crate::version::Version;
use autonomi::PackageVersion;

/// Start minimum node package version to be eligible for rewards.
const START_MIN_VERSION: PackageVersion = PackageVersion {
    year: 2025,
    month: 4,
    cycle: 1,
    cycle_counter: 1,
};

const START_ANTNODE_VERSION: Version = Version {
    major: 0,
    minor: 3,
    patch: 10,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionPack {
    pub package_version: PackageVersion,
    pub antnode_version: Version,
}

impl Default for VersionPack {
    fn default() -> Self {
        Self {
            package_version: START_MIN_VERSION,
            antnode_version: START_ANTNODE_VERSION,
        }
    }
}
