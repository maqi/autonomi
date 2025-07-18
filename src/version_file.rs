use autonomi::networking::version::PackageVersion;
use eyre::Result;
use serde::Deserialize;

const VERSION_FILE_URL: &str =
    "https://raw.githubusercontent.com/maidsafe/gists/refs/heads/main/rewards-service-min-package-version.json";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionFileContent {
    package_version: String,
}

pub async fn fetch_min_package_version() -> Result<PackageVersion> {
    let response = reqwest::get(VERSION_FILE_URL).await?;
    let content: VersionFileContent = response.json().await?;
    PackageVersion::try_from(content.package_version)
        .map_err(|err| eyre::eyre!("Failed to parse package version: {}", err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_min_package_version() {
        let result = fetch_min_package_version().await;
        assert!(result.is_ok());
    }
}
