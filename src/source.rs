use crate::{CancellationToken, Error, Result};
use reqwest::{
    Url,
    blocking::Client,
    header::{ACCEPT, USER_AGENT},
};
use serde::{Deserialize, de::DeserializeOwned};
use std::time::Duration;

/// GitHub's single-page ceiling. Release lookup never paginates.
pub const MAX_PER_PAGE: usize = 100;

/// The subset of a forge release used by channel resolution.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Release {
    /// Original release tag, passed unchanged to the installer.
    pub tag: String,
    /// Whether the forge marks this as a prerelease.
    pub prerelease: bool,
}

/// A release provider. Lists must be newest first and exclude drafts.
pub trait Source {
    /// Read the forge's latest release.
    fn latest(&self, token: &CancellationToken) -> Result<Release>;
    /// Read one page, preserving newest-first order.
    fn list(&self, token: &CancellationToken, limit: usize) -> Result<Vec<Release>>;
}
impl<T: Source + ?Sized> Source for &T {
    fn latest(&self, token: &CancellationToken) -> Result<Release> {
        (**self).latest(token)
    }
    fn list(&self, token: &CancellationToken, limit: usize) -> Result<Vec<Release>> {
        (**self).list(token, limit)
    }
}
impl<T: Source + ?Sized> Source for Box<T> {
    fn latest(&self, token: &CancellationToken) -> Result<Release> {
        (**self).latest(token)
    }
    fn list(&self, token: &CancellationToken, limit: usize) -> Result<Vec<Release>> {
        (**self).list(token, limit)
    }
}

/// Blocking GitHub release access, with a 30-second default request timeout.
#[derive(Clone)]
pub struct GitHubSource {
    releases_url: Url,
    token: Option<String>,
    client: Client,
    timeout: Duration,
}

/// GitHub connection configuration. Tokens are intentionally excluded from Debug.
pub struct GitHubSourceBuilder {
    owner: String,
    repo: String,
    api_base: String,
    token: Option<String>,
    client: Option<Client>,
    timeout: Duration,
}

impl GitHubSource {
    /// Construct a source for public GitHub using default client settings.
    pub fn new(owner: impl Into<String>, repo: impl Into<String>) -> Result<Self> {
        Self::builder(owner, repo).build()
    }
    /// Configure GitHub or a compatible API endpoint.
    pub fn builder(owner: impl Into<String>, repo: impl Into<String>) -> GitHubSourceBuilder {
        GitHubSourceBuilder {
            owner: owner.into(),
            repo: repo.into(),
            api_base: "https://api.github.com".into(),
            token: None,
            client: None,
            timeout: Duration::from_secs(30),
        }
    }

    fn get<T: DeserializeOwned>(&self, token: &CancellationToken, url: Url) -> Result<T> {
        token.check()?;
        let path = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().into(),
        };
        let mut request = self
            .client
            .get(url)
            .timeout(self.timeout)
            .header(ACCEPT, "application/vnd.github+json")
            .header(
                USER_AGENT,
                concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
            );
        if let Some(auth) = &self.token {
            request = request.bearer_auth(auth);
        }
        token.check()?;
        let response = request.send();
        token.check()?;
        let response = response.map_err(Error::Http)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::HttpStatus {
                path,
                status: response.status().as_u16(),
            });
        }
        let bytes = response.bytes();
        token.check()?;
        let result = serde_json::from_slice(&bytes.map_err(Error::Http)?)
            .map_err(|source| Error::Decode { path, source });
        token.check()?;
        result
    }
}

impl GitHubSourceBuilder {
    /// Override the API base, including an optional path such as `/api/v3`.
    pub fn api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }
    /// Send an optional bearer token. Empty strings omit the header.
    pub fn token(mut self, token: impl Into<String>) -> Self {
        let token = token.into();
        self.token = if token.is_empty() { None } else { Some(token) };
        self
    }
    /// Supply a blocking Reqwest client for proxy, TLS, or other transport settings.
    /// The source's request timeout and required headers still apply.
    pub fn client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }
    /// Set a nonzero timeout covering each request, including its response body.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    /// Validate configuration and construct the client.
    /// Call outside an async runtime, as required by blocking Reqwest.
    pub fn build(self) -> Result<GitHubSource> {
        if self.timeout.is_zero() {
            return Err(Error::Configuration(
                "request timeout must be nonzero".into(),
            ));
        }
        if self.owner.is_empty() || self.repo.is_empty() {
            return Err(Error::Configuration(
                "GitHub owner and repository must be nonempty".into(),
            ));
        }
        let mut url = Url::parse(&self.api_base).map_err(Error::external)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Configuration(
                "API base must be an HTTP(S) URL without a query or fragment".into(),
            ));
        }
        url.path_segments_mut()
            .map_err(|()| Error::Configuration("API base cannot contain path segments".into()))?
            .pop_if_empty()
            .extend(["repos", &self.owner, &self.repo, "releases"]);
        let client = match self.client {
            Some(client) => client,
            None => Client::builder()
                .timeout(self.timeout)
                .build()
                .map_err(Error::Http)?,
        };
        Ok(GitHubSource {
            releases_url: url,
            token: self.token,
            client,
            timeout: self.timeout,
        })
    }
}

#[derive(Deserialize)]
struct GitHubRelease {
    // Flattened fields require a JSON object; a positional JSON array is not a
    // GitHub release, even though Serde normally accepts arrays for structs.
    #[serde(flatten)]
    fields: GitHubReleaseFields,
}

#[derive(Deserialize)]
struct GitHubReleaseFields {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}
impl GitHubRelease {
    fn into_release(self) -> Release {
        Release {
            tag: self.fields.tag_name,
            prerelease: self.fields.prerelease,
        }
    }
}
impl Source for GitHubSource {
    fn latest(&self, token: &CancellationToken) -> Result<Release> {
        let mut url = self.releases_url.clone();
        url.path_segments_mut()
            .expect("validated HTTP URL")
            .push("latest");
        let release: GitHubRelease = self.get(token, url)?;
        // GitHub should never return a draft from latest; do not offer one if it does.
        Ok(if release.fields.draft {
            Release::default()
        } else {
            release.into_release()
        })
    }
    fn list(&self, token: &CancellationToken, limit: usize) -> Result<Vec<Release>> {
        let limit = if (1..=MAX_PER_PAGE).contains(&limit) {
            limit
        } else {
            MAX_PER_PAGE
        };
        let mut url = self.releases_url.clone();
        url.query_pairs_mut()
            .append_pair("per_page", &limit.to_string());
        let releases: Vec<GitHubRelease> = self.get(token, url)?;
        Ok(releases
            .into_iter()
            .filter(|r| !r.fields.draft)
            .map(GitHubRelease::into_release)
            .collect())
    }
}
