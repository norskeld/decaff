use std::collections::HashMap;
use std::fmt::{self, Display};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use git2::build::CheckoutBuilder;
use git2::Repository as GitRepository;
use miette::{Diagnostic, LabeledSpan, Report};
use thiserror::Error;

use crate::path::Traverser;

/// Helper macro to create a [ParseError] in a slightly less verbose way.
macro_rules! parse_error {
  ($source:ident = $code:expr, $($key:ident = $value:expr,)* $fmt:literal $($arg:tt)*) => {
    ParseError(
      miette::Report::from(
        miette::diagnostic!($($key = $value,)* $fmt $($arg)*)
      ).with_source_code($code)
    )
  };
}

#[derive(Debug, Diagnostic, Error)]
pub enum RepositoryError {
  #[error("{message}")]
  #[diagnostic(code(decaff::repository::io))]
  Io {
    message: String,
    #[source]
    source: io::Error,
  },
}

#[derive(Debug, Diagnostic, Error)]
#[error("{0}")]
#[diagnostic(transparent)]
pub struct ParseError(Report);

#[derive(Debug, Diagnostic, Error)]
#[diagnostic(code(decaff::repository::fetch))]
pub enum FetchError {
  #[error("Request failed.")]
  RequestFailed,
  #[error("Repository download failed with code {code}. {report}")]
  RequestFailedWithCode { code: u16, report: Report },
  #[error("Couldn't get the response body as bytes.")]
  RequestBodyFailed,
}

#[derive(Debug, Diagnostic, Error)]
#[diagnostic(code(decaff::repository::remote))]
pub enum RemoteError {
  #[error("Failed to create a detached in-memory remote.\n\n{url}")]
  CreateDetachedRemoteFailed { url: Report },
  #[error("Failed to connect the given remote.\n\n{url}")]
  ConnectionFailed { url: Report },
}

#[derive(Debug, Diagnostic, Error)]
#[diagnostic(code(decaff::repository::reference))]
pub enum ReferenceError {
  #[error("Invalid reference: `{0}`.")]
  InvalidSelector(String),
}

#[derive(Debug, Diagnostic, Error)]
#[diagnostic(code(decaff::repository::checkout))]
pub enum CheckoutError {
  #[error("Failed to open the git repository.")]
  OpenFailed(git2::Error),
  #[error("Failed to parse revision string `{0}`.")]
  RevparseFailed(String),
  #[error("Failed to checkout revision (tree).")]
  TreeCheckoutFailed,
  #[error("Reference name is not a valid UTF-8 string.")]
  InvalidRefName,
  #[error("Failed to set HEAD to `{0}`.")]
  SetHeadFailed(String),
  #[error("Failed to detach HEAD to `{0}`.")]
  DetachHeadFailed(String),
}

/// Supported hosts. [GitHub][RepositoryHost::GitHub] is the default one.
#[derive(Debug, Default, PartialEq)]
pub enum RepositoryHost {
  #[default]
  GitHub,
  GitLab,
  BitBucket,
}

impl Display for RepositoryHost {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let host = match self {
      | RepositoryHost::GitHub => "github",
      | RepositoryHost::GitLab => "gitlab",
      | RepositoryHost::BitBucket => "bitbucket",
    };

    write!(f, "{host}")
  }
}

/// Repository meta or *ref*, i.e. branch, tag or commit hash.
///
/// This newtype exists solely for providing the default value.
#[derive(Clone, Debug, PartialEq)]
pub struct RepositoryMeta(pub String);

impl Default for RepositoryMeta {
  fn default() -> Self {
    // Using "HEAD" instead of hardcoding the default branch name like "master" or "main".
    // Suprisingly, works just fine.
    Self("HEAD".to_string())
  }
}

impl Display for RepositoryMeta {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}", self.0)
  }
}

/// Represents a remote repository. Repositories of this kind need to be downloaded first.
#[derive(Debug, PartialEq)]
pub struct RemoteRepository {
  pub host: RepositoryHost,
  pub user: String,
  pub repo: String,
  pub meta: RepositoryMeta,
  pub refs: HashMap<String, String>,
}

impl RemoteRepository {
  /// Creates new `RemoteRepository`.
  pub fn new(target: String, meta: Option<String>) -> Result<Self, ParseError> {
    let repo = Self::from_str(&target)?;
    let meta = meta.map_or(repo.meta, RepositoryMeta);

    Ok(Self { meta, ..repo })
  }

  /// Resolves a URL depending on the host and other repository fields.
  pub fn get_tar_url(&self) -> String {
    let RemoteRepository { host, user, repo, meta, .. } = self;

    match host {
      | RepositoryHost::GitHub => {
        format!("https://github.com/{user}/{repo}/archive/{meta}.tar.gz")
      },
      | RepositoryHost::GitLab => {
        format!("https://gitlab.com/{user}/{repo}/-/archive/{meta}/{repo}.tar.gz")
      },
      | RepositoryHost::BitBucket => {
        format!("https://bitbucket.org/{user}/{repo}/get/{meta}.tar.gz")
      },
    }
  }

  /// Resolves a git repository URL depending on the host and other repository fields.
  pub fn get_git_url(&self) -> String {
    let RemoteRepository { host, user, repo, .. } = self;

    match host {
      | RepositoryHost::GitHub => format!("https://github.com/{user}/{repo}.git"),
      | RepositoryHost::GitLab => format!("https://gitlab.com/{user}/{repo}.git"),
      | RepositoryHost::BitBucket => format!("https://bitbucket.org/{user}/{repo}.git"),
    }
  }

  /// Returns the source string of the repository.
  pub fn get_source(&self) -> String {
    let host = match self.host {
      | RepositoryHost::GitHub => "github",
      | RepositoryHost::GitLab => "gitlab",
      | RepositoryHost::BitBucket => "bitbucket",
    };

    let user = &self.user;
    let repo = &self.repo;

    format!("{host}:{user}/{repo}")
  }

  /// Fetches the refs of the remote repository.
  pub fn fetch_refs(&mut self) -> Result<(), RemoteError> {
    let git_url = self.get_git_url();

    let mut remote = git2::Remote::create_detached(git_url.as_bytes()).map_err(|_| {
      RemoteError::CreateDetachedRemoteFailed { url: miette::miette!("URL: {git_url}") }
    })?;

    let connection = remote
      .connect_auth(git2::Direction::Fetch, None, None)
      .map_err(|_| RemoteError::ConnectionFailed { url: miette::miette!("URL: {git_url}") })?;

    for head in connection.list().unwrap() {
      let original = head.name();

      let name = (original == "HEAD")
        .then_some("HEAD")
        .or_else(|| original.strip_prefix("refs/heads/"))
        .or_else(|| original.strip_prefix("refs/tags/"))
        .map(str::to_string);

      if let Some(name) = name {
        self.refs.insert(name, head.oid().to_string());
      }
    }

    Ok(())
  }

  /// Resolves a given reference to a commit hash.
  pub fn resolve_hash(&self) -> Result<String, ReferenceError> {
    let selector = self.meta.to_string();

    // If selector is a branch or tag.
    if let Some(hash) = self.refs.get(&selector) {
      Ok(hash.to_owned())
    }
    // Or it might be a (short) commit hash.
    else if selector.len() >= 7 {
      git2::Oid::from_str(&selector)
        .map(|oid| {
          let oid = oid.to_string();

          // Try to find a full commit hash.
          if let Some(full_hash) = self.refs.values().find(|hash| hash.starts_with(&oid)) {
            full_hash.to_owned()
          }
          // At this point this is most likely a commit that's not a tip of any branch.
          else {
            selector.clone()
          }
        })
        .map_err(|_| ReferenceError::InvalidSelector(selector))
    }
    // Otherwise this is not a valid ref.
    else {
      Err(ReferenceError::InvalidSelector(selector))
    }
  }

  /// Fetches the tarball using the resolved URL, and reads it into a vector of bytes.
  pub async fn fetch(&self) -> Result<Vec<u8>, FetchError> {
    let url = self.get_tar_url();

    let response = reqwest::get(&url).await.map_err(|err| {
      err.status().map_or(FetchError::RequestFailed, |status| {
        FetchError::RequestFailedWithCode {
          code: status.as_u16(),
          report: miette::miette!("\n\nURL: {}", url.clone()),
        }
      })
    })?;

    let status = response.status();

    if !status.is_success() {
      let code = status.as_u16();

      let report = if code == 404 {
        miette::miette!("The requested branch, tag or commit was not found.\n\nURL: {url}")
      } else {
        miette::miette!("\n\nURL: {url}")
      };

      return Err(FetchError::RequestFailedWithCode { code, report });
    }

    response
      .bytes()
      .await
      .map(|bytes| bytes.to_vec())
      .map_err(|_| FetchError::RequestBodyFailed)
  }
}

impl FromStr for RemoteRepository {
  type Err = ParseError;

  /// Parses a `&str` into a `RemoteRepository`.
  fn from_str(input: &str) -> Result<Self, Self::Err> {
    #[inline(always)]
    fn is_valid_user(ch: char) -> bool {
      ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
    }

    #[inline(always)]
    fn is_valid_repo(ch: char) -> bool {
      is_valid_user(ch) || ch == '.'
    }

    let source = input.trim();

    // Parse host if present or use default otherwise.
    let (host, (input, offset)) = if let Some((host, rest)) = source.split_once(':') {
      let host = host.to_ascii_lowercase();
      let next_offset = host.len() + 1;

      match host.as_str() {
        | "github" | "gh" => (RepositoryHost::GitHub, (rest, next_offset)),
        | "gitlab" | "gl" => (RepositoryHost::GitLab, (rest, next_offset)),
        | "bitbucket" | "bb" => (RepositoryHost::BitBucket, (rest, next_offset)),
        | _ => {
          return Err(parse_error!(
            source = source.to_string(),
            code = "decaff::repository::parse",
            labels = vec![LabeledSpan::at(
              (0, host.len()),
              "must be one of: github/gh, gitlab/gl, or bitbucket/bb"
            )],
            "Invalid host: `{host}`."
          ));
        },
      }
    } else {
      (RepositoryHost::default(), (source, 0))
    };

    // Parse user name.
    let (user, (input, offset)) = if let Some((user, rest)) = input.split_once('/') {
      let next_offset = offset + user.len() + 1;

      if user.chars().all(is_valid_user) {
        (user.to_string(), (rest, next_offset))
      } else {
        return Err(parse_error!(
          source = source.to_string(),
          code = "decaff::repository::parse",
          labels = vec![LabeledSpan::at(
            (offset, user.len()),
            "only ASCII alphanumeric characters, _ and - allowed"
          )],
          "Invalid user name: `{user}`."
        ));
      }
    } else {
      return Err(ParseError(miette::miette!("Missing repository name.")));
    };

    // Short-circuit if the rest of the input contains another /.
    if let Some(slash_idx) = input.find('/') {
      // Ensure we are not triggering false-positive in case there's a ref (after #) with a branch
      // name containing slashes.
      if matches!(input.find('#'), Some(hash_idx) if slash_idx < hash_idx) {
        return Err(parse_error!(
          source = source.to_string(),
          code = "decaff::repository::parse",
          labels = vec![LabeledSpan::at((offset + slash_idx, 1), "remove this")],
          "Multiple slashes in the input."
        ));
      }
    }

    // Parse repository name.
    let (repo, input) = input.split_once('#').map_or_else(
      || (input.to_string(), None),
      |(repo, rest)| (repo.to_string(), Some(rest)),
    );

    if !repo.chars().all(is_valid_repo) {
      return Err(parse_error!(
        source = source.to_string(),
        code = "decaff::repository::parse",
        labels = vec![LabeledSpan::at(
          (offset, repo.len()),
          "only ASCII alphanumeric characters, _, - and . allowed"
        ),],
        "Invalid repository name: `{repo}`."
      ));
    }

    // Produce meta if anything left from the input. Empty meta is accepted but ignored, default
    // value is used then.
    let meta = input
      .filter(|input| !input.is_empty())
      .map_or(RepositoryMeta::default(), |input| {
        RepositoryMeta(input.to_string())
      });

    let refs = HashMap::default();

    Ok(RemoteRepository { host, user, repo, meta, refs })
  }
}

/// Represents a local repository.
///
/// Repositories of this kind don't need to be downloaded, we can:
/// - if a git repository — simply clone it locally and switch to desired meta (ref);
/// - if a directory — simply copy it as-is.
#[derive(Debug, PartialEq)]
pub struct LocalRepository {
  pub source: PathBuf,
  pub meta: RepositoryMeta,
}

impl LocalRepository {
  /// Creates new `LocalRepository`.
  pub fn new(source: String, meta: Option<String>) -> Self {
    Self {
      source: PathBuf::from(source),
      meta: meta.map_or(RepositoryMeta::default(), RepositoryMeta),
    }
  }

  /// Copies the repository into the `destination` directory.
  pub fn copy(&self, destination: &Path) -> Result<(), RepositoryError> {
    let traverser = Traverser::new(self.source.to_owned())
      .pattern("**/*")
      .ignore_dirs(true)
      .contents_first(true);

    for matched in traverser.iter().flatten() {
      let target = destination.join(&matched.captured);

      if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|source| {
          RepositoryError::Io {
            message: format!(
              "Failed to create directory structure for '{}'.",
              parent.display()
            ),
            source,
          }
        })?;

        fs::copy(&matched.path, &target).map_err(|source| {
          RepositoryError::Io {
            message: format!(
              "Failed to copy from '{}' to '{}'.",
              matched.path.display(),
              target.display()
            ),
            source,
          }
        })?;
      }
    }

    Ok(())
  }

  /// Checks out the repository located at the `destination`.
  pub fn checkout(&self, destination: &Path) -> Result<(), CheckoutError> {
    let meta = self.meta.to_string();
    let head = "HEAD".to_string();

    // First, try to create Repository.
    let repository = GitRepository::open(destination).map_err(CheckoutError::OpenFailed)?;

    // Note: in case of local repositories, instead of HEAD we want to check origin/HEAD first,
    // which should be the default branch if the repository has been cloned from a remote.
    // Otherwise we fallback to HEAD, which will point to whatever the repository points at the time
    // of cloning (can be absolutely arbitrary reference/state).
    let meta = if meta == "HEAD" {
      repository
        .revparse_ext("origin/HEAD")
        .ok()
        .and_then(|(_, reference)| reference)
        .and_then(|reference| reference.name().map(str::to_string))
        .unwrap_or(head)
    } else {
      head
    };

    // Try to find (parse revision) the desired reference: branch, tag or commit. They are encoded
    // in two objects:
    //
    // - `object` contains (among other things) the commit hash.
    // - `reference` points to the branch or tag.
    let (object, reference) = repository
      .revparse_ext(&meta)
      .map_err(|_| CheckoutError::RevparseFailed(meta))?;

    // Build checkout options.
    let mut checkout = CheckoutBuilder::new();

    checkout
      .skip_unmerged(true)
      .remove_untracked(true)
      .remove_ignored(true)
      .force();

    // Updates files in the index and working tree.
    repository
      .checkout_tree(&object, Some(&mut checkout))
      .map_err(|_| CheckoutError::TreeCheckoutFailed)?;

    match reference {
      // Here `gref` is an actual reference like branch or tag.
      | Some(gref) => {
        let ref_name = gref.name().ok_or(CheckoutError::InvalidRefName)?;

        repository
          .set_head(ref_name)
          .map_err(|_| CheckoutError::SetHeadFailed(ref_name.to_string()))?;
      },
      // This is a commit, detach HEAD.
      | None => {
        let hash = object.id();

        repository
          .set_head_detached(hash)
          .map_err(|_| CheckoutError::DetachHeadFailed(hash.to_string()))?;
      },
    }

    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parse_remote_default() {
    assert_eq!(
      RemoteRepository::from_str("foo/bar").map_err(|report| report.to_string()),
      Ok(RemoteRepository {
        host: RepositoryHost::GitHub,
        user: "foo".to_string(),
        repo: "bar".to_string(),
        meta: RepositoryMeta::default(),
        refs: HashMap::default()
      })
    );
  }

  #[test]
  fn parse_remote_missing_reponame() {
    assert_eq!(
      RemoteRepository::from_str("foo-bar").map_err(|report| report.to_string()),
      Err("Missing repository name.".to_string())
    );
  }

  #[test]
  fn parse_remote_invalid_username() {
    assert_eq!(
      RemoteRepository::from_str("foo@bar/baz").map_err(|report| report.to_string()),
      Err("Invalid user name: `foo@bar`.".to_string())
    );
  }

  #[test]
  fn parse_remote_invalid_reponame() {
    assert_eq!(
      RemoteRepository::from_str("foo-bar/b@z").map_err(|report| report.to_string()),
      Err("Invalid repository name: `b@z`.".to_string())
    );
  }

  #[test]
  fn parse_remote_invalid_host() {
    assert_eq!(
      RemoteRepository::from_str("srht:foo/bar").map_err(|report| report.to_string()),
      Err(
        parse_error!(
          source = "srht:foo/bar",
          code = "decaff::repository::parse",
          labels = vec![LabeledSpan::at(
            (0, 5),
            "must be one of: github/gh, gitlab/gl, or bitbucket/bb"
          )],
          "Invalid host: `srht`."
        )
        .to_string()
      )
    );
  }

  #[test]
  fn parse_remote_meta() {
    let cases = [
      ("foo/bar", RepositoryMeta::default()),
      ("foo/bar#foo", RepositoryMeta("foo".to_string())),
      ("foo/bar#4a5a56fd", RepositoryMeta("4a5a56fd".to_string())),
      (
        "foo/bar#feat/some-feature-name",
        RepositoryMeta("feat/some-feature-name".to_string()),
      ),
    ];

    for (input, meta) in cases {
      assert_eq!(
        RemoteRepository::from_str(input).map_err(|report| report.to_string()),
        Ok(RemoteRepository {
          host: RepositoryHost::GitHub,
          user: "foo".to_string(),
          repo: "bar".to_string(),
          refs: HashMap::default(),
          meta,
        })
      );
    }
  }

  #[test]
  fn parse_remote_hosts() {
    let cases = [
      ("github:foo/bar", RepositoryHost::GitHub),
      ("gh:foo/bar", RepositoryHost::GitHub),
      ("gitlab:foo/bar", RepositoryHost::GitLab),
      ("gl:foo/bar", RepositoryHost::GitLab),
      ("bitbucket:foo/bar", RepositoryHost::BitBucket),
      ("bb:foo/bar", RepositoryHost::BitBucket),
    ];

    for (input, host) in cases {
      assert_eq!(
        RemoteRepository::from_str(input).map_err(|report| report.to_string()),
        Ok(RemoteRepository {
          host,
          user: "foo".to_string(),
          repo: "bar".to_string(),
          meta: RepositoryMeta::default(),
          refs: HashMap::default()
        })
      );
    }
  }

  #[test]
  fn test_remote_empty_meta() {
    assert_eq!(
      RemoteRepository::from_str("foo/bar#").map_err(|report| report.to_string()),
      Ok(RemoteRepository {
        host: RepositoryHost::GitHub,
        user: "foo".to_string(),
        repo: "bar".to_string(),
        meta: RepositoryMeta::default(),
        refs: HashMap::default()
      })
    );
  }

  #[test]
  fn parse_remote_ambiguous_username() {
    let cases = [
      ("github/foo", "github", "foo"),
      ("gh/foo", "gh", "foo"),
      ("gitlab/foo", "gitlab", "foo"),
      ("gl/foo", "gl", "foo"),
      ("bitbucket/foo", "bitbucket", "foo"),
      ("bb/foo", "bb", "foo"),
    ];

    for (input, user, repo) in cases {
      assert_eq!(
        RemoteRepository::from_str(input).map_err(|report| report.to_string()),
        Ok(RemoteRepository {
          host: RepositoryHost::default(),
          user: user.to_string(),
          repo: repo.to_string(),
          meta: RepositoryMeta::default(),
          refs: HashMap::default()
        })
      );
    }
  }
}
