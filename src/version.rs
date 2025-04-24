use std::fmt::Display;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    /// Major version.
    pub major: u16,
    /// Minor version.
    pub minor: u16,
    /// Patch version.
    pub patch: u16,
}

impl Version {
    pub fn new(major: u16, minor: u16, patch: u16) -> Self {
        Version {
            major,
            minor,
            patch,
        }
    }

    pub fn is_minimum(&self, other: &Version) -> bool {
        (self.major, self.minor, self.patch) >= (other.major, other.minor, other.patch)
    }

    pub fn is_maximum(&self, other: &Version) -> bool {
        (self.major, self.minor, self.patch) <= (other.major, other.minor, other.patch)
    }

    pub fn is_exact(&self, other: &Version) -> bool {
        self.major == other.major && self.minor == other.minor && self.patch == other.patch
    }
}

impl TryFrom<String> for Version {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parts: Vec<&str> = value.split('.').collect();
        if parts.len() != 3 {
            return Err("Invalid version format. Expected major.minor.patch".to_string());
        }

        let major = parts[0]
            .parse::<u16>()
            .map_err(|_| "Failed to parse major version as u16".to_string())?;
        let minor = parts[1]
            .parse::<u16>()
            .map_err(|_| "Failed to parse minor version as u16".to_string())?;
        let patch = parts[2]
            .parse::<u16>()
            .map_err(|_| "Failed to parse patch version as u16".to_string())?;

        Ok(Version {
            major,
            minor,
            patch,
        })
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_minimum() {
        let version1 = Version::new(1, 2, 0);
        let version2 = Version::new(1, 3, 0);
        let version3 = Version::new(1, 2, 1);
        let version4 = Version::new(0, 9, 5);
        let version5 = Version::new(1, 2, 0);

        assert!(version1.is_minimum(&version4));
        assert!(!version1.is_minimum(&version2));
        assert!(version1.is_minimum(&version1));
        assert!(!version1.is_minimum(&version3));
        assert!(version1.is_minimum(&version5));
    }

    #[test]
    fn test_is_maximum() {
        let version1 = Version::new(1, 2, 0);
        let version2 = Version::new(1, 3, 0);
        let version3 = Version::new(1, 1, 9);
        let version4 = Version::new(2, 0, 0);
        let version5 = Version::new(1, 2, 1);

        assert!(version1.is_maximum(&version1));
        assert!(!version1.is_maximum(&version3));
        assert!(version1.is_maximum(&version2));
        assert!(version1.is_maximum(&version4));
        assert!(version1.is_maximum(&version5));
    }

    #[test]
    fn test_is_exact() {
        let version1 = Version::new(1, 2, 3);
        let version2 = Version::new(1, 2, 3);
        let version3 = Version::new(1, 2, 4);
        assert!(version1.is_exact(&version2));
        assert!(!version1.is_exact(&version3));
    }

    #[test]
    fn test_try_from_valid_string() {
        let version = Version::try_from("1.2.3".to_string()).unwrap();
        assert_eq!(version.major, 1);
        assert_eq!(version.minor, 2);
        assert_eq!(version.patch, 3);
    }

    #[test]
    fn test_try_from_invalid_string() {
        let result = Version::try_from("1.2".to_string());
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Invalid version format. Expected major.minor.patch"
        );
    }

    #[test]
    fn test_display() {
        let version = Version::new(1, 2, 3);
        assert_eq!(version.to_string(), "1.2.3");
    }
}
