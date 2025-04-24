use crate::version::Version;
use autonomi::PackageVersion;
use eyre::Result;
use octocrab::models::repos::Release;
use octocrab::Octocrab;
use regex::Regex;
use std::sync::OnceLock;

static STABLE_TAG_REGEX: OnceLock<Regex> = OnceLock::new();
static ANTNODE_VERSION_REGEX: OnceLock<Regex> = OnceLock::new();

fn stable_tag_regex() -> &'static Regex {
    STABLE_TAG_REGEX.get_or_init(|| {
        Regex::new(r"^stable-\d{4}\.\d{1,2}\.\d{1,2}\.\d{1,2}$").expect("Invalid regex")
    })
}

fn antnode_version_regex() -> &'static Regex {
    ANTNODE_VERSION_REGEX
        .get_or_init(|| Regex::new(r"(?m)`antnode`:\sv(\d+\.\d+\.\d+)").expect("Invalid regex"))
}

async fn get_latest_stable_release() -> Result<Release> {
    const OWNER: &str = "maidsafe";
    const REPO: &str = "autonomi";

    let github = Octocrab::builder().build()?;
    let release = github.repos(OWNER, REPO).releases().get_latest().await?;

    if stable_tag_regex().is_match(&release.tag_name) {
        return Ok(release);
    }

    Err(eyre::eyre!("Could not find latest stable release."))
}

fn grep_antnode_version_from_release_body(body: &str) -> Option<Version> {
    let regex = antnode_version_regex();

    if let Some(captures) = regex.captures(body) {
        if let Some(version) = captures.get(1) {
            return Version::try_from(version.as_str().to_string()).ok();
        }
    }

    None
}

pub async fn get_latest_package_and_antnode_version() -> Result<(PackageVersion, Option<Version>)> {
    let release = get_latest_stable_release().await?;

    let version_str = release
        .tag_name
        .strip_prefix("stable-")
        .ok_or(eyre::eyre!("Invalid release tag."))?;

    let package_version =
        PackageVersion::try_from(version_str.to_string()).map_err(|err| eyre::eyre!(err))?;

    let maybe_antnode_version =
        grep_antnode_version_from_release_body(&release.body.unwrap_or_default());

    Ok((package_version, maybe_antnode_version))
}

#[cfg(test)]
mod tests {
    use crate::github::get_latest_package_and_antnode_version;

    #[tokio::test]
    async fn test_get_latest_package_and_antnode_version() {
        let result = get_latest_package_and_antnode_version().await.unwrap();
        println!("{result:?}");
    }
}
