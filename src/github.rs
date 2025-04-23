use autonomi::PackageVersion;
use eyre::Result;
use octocrab::Octocrab;
use regex::Regex;
use std::sync::OnceLock;

static STABLE_TAG_REGEX: OnceLock<Regex> = OnceLock::new();

fn stable_tag_regex() -> &'static Regex {
    STABLE_TAG_REGEX.get_or_init(|| {
        Regex::new(r"^stable-\d{4}\.\d{1,2}\.\d{1,2}\.\d{1,2}$").expect("Invalid regex")
    })
}

pub async fn get_latest_stable_release() -> Result<Option<PackageVersion>> {
    const OWNER: &str = "maidsafe";
    const REPO: &str = "autonomi";

    let github = Octocrab::builder().build()?;
    let release = github.repos(OWNER, REPO).releases().get_latest().await?;

    if stable_tag_regex().is_match(&release.tag_name) {
        if let Some(version_str) = release.tag_name.strip_prefix("stable-") {
            return PackageVersion::try_from(version_str.to_string())
                .map(Some)
                .map_err(|err| eyre::eyre!(err));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use crate::github::get_latest_stable_release;

    #[tokio::test]
    async fn test_fetch_all_tags() {
        let result = get_latest_stable_release().await.unwrap();
        println!("{result:?}");
    }
}
