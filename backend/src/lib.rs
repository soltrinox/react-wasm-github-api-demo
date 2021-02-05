use serde::{Deserialize, Serialize};
mod utils;
use cfg_if::cfg_if;
use graphql_client::GraphQLQuery;

// Github Schema DateTime type is just a string
type DateTime = String;
type URI = String;
type GitObjectID = String;

#[derive(GraphQLQuery)]
#[graphql(schema_path = "schema.json", query_path = "src/head-query.graphql")]
pub struct BranchHeadCommitAuthor;

use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode};

#[derive(Debug, Serialize, Deserialize)]
pub struct User {
    avatar_url: URI,
    handle: Option<String>,
    name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Repo {
    name_with_owner: String,
    owner: User,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Branch {
    name: String,
    head: Commit,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Commit {
    author: User,
    committer: User,
    sha: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphqlError {
    message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FatalError {
    message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RateLimitInfo {
    cost: i64,
    limit: i64,
    node_count: i64,
    remaining: i64,
    used: i64,
    reset_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Data {
    errors: Vec<GraphqlError>,
    rate_limit_info: RateLimitInfo,
    branch: Branch,
    repo: Repo,
}

cfg_if! {
    if #[cfg(feature = "console_log")] {
        fn init_log() {
            console_log::init_with_level(log::Level::Trace).expect("error initializing console logging");
        }
    } else {
        fn init_log() {}
    }
}

// When the `wee_alloc` feature is enabled, use `wee_alloc` as the global
// allocator.
#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[derive(Debug, Serialize)]
struct GithubHeaders {
    pub authorization: String,
    pub accept: String,
}

impl From<FatalError> for JsValue {
    fn from(err: FatalError) -> Self {
        JsValue::from_serde(&err).unwrap()
    }
}

impl From<JsValue> for FatalError {
    fn from(js: JsValue) -> Self {
        FatalError {
            message: format!("JS error: {:#?}", js),
        }
    }
}

impl From<serde_json::Error> for FatalError {
    fn from(err: serde_json::Error) -> Self {
        FatalError {
            message: format!("De/serialisation error: {}", err),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub enum GraphqlResult {
    Data(Data),
    Error(FatalError),
}

impl From<GraphqlResult> for JsValue {
    fn from(res: GraphqlResult) -> Self {
        JsValue::from_serde(&res).unwrap()
    }
}

#[wasm_bindgen]
pub async fn run_graphql(
    owner: String,
    repo: String,
    branch: String,
    token: String,
) -> GraphqlResult {
    match run_graphql_private(owner, repo, branch, token).await {
        Ok(data) => GraphqlResult::Data(data),
        Err(err) => GraphqlResult::Error(err),
    }
}

async fn run_graphql_private(
    owner: String,
    repo: String,
    branch: String,
    token: String,
) -> Result<Data, FatalError> {
    init_log();
    let query = BranchHeadCommitAuthor::build_query(branch_head_commit_author::Variables {
        branch: branch.clone(),
        owner,
        repo_name: repo,
    });
    let mut opts = RequestInit::new();
    let headers = JsValue::from_serde(&GithubHeaders {
        authorization: format!("Bearer {}", token),
        accept: "application/vnd.github.v3+json".into(),
    })?;
    opts.headers(&headers);
    opts.method("POST");
    opts.mode(RequestMode::Cors);
    let body = serde_json::to_string(&query)?;
    opts.body(Option::Some(&JsValue::from_str(&body)));

    let url = "https://api.github.com/graphql";

    let window = web_sys::window().expect("Should have window here");
    let request = Request::new_with_str_and_init(&url, &opts)?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request)).await?;
    let cast_response = resp_value.dyn_into::<web_sys::Response>()?;
    let text_response = cast_response.text()?;
    let text = JsFuture::from(text_response).await?;
    let raw_response = text.as_string().ok_or(FatalError {
        message: "Couldn't get response text.".into(),
    })?;
    log::debug!("{:#?}", raw_response);

    let resp_data_option: graphql_client::Response<branch_head_commit_author::ResponseData> =
        serde_json::from_str(&raw_response)?;

    let data = resp_data_option.data.ok_or(FatalError {
        message: "Could not get response data".into(),
    })?;
    let rate_limit = data.rate_limit.ok_or(FatalError {
        message: "Could not find rate limit info in data.".into(),
    })?;
    let repository: branch_head_commit_author::BranchHeadCommitAuthorRepository =
        data.repository.ok_or(FatalError {
            message: "Could not find repository info in data.".into(),
        })?;
    let branch_ref = repository.ref_.ok_or(FatalError {
        message: format!(
            "No branch {} on repository {}",
            &branch, repository.name_with_owner
        ),
    })?;
    let head = branch_ref.target.ok_or(FatalError {
        message: format!(
            "No target on for branch {} on repository {}",
            &branch, repository.name_with_owner
        ),
    })?;

    Ok(Data {
        rate_limit_info: RateLimitInfo {
            cost: rate_limit.cost,
            limit: rate_limit.limit,
            node_count: rate_limit.node_count,
            remaining: rate_limit.remaining,
            used: rate_limit.used,
            reset_at: rate_limit.reset_at,
        },
        repo: Repo {
            name_with_owner: repository.name_with_owner,
            owner: get_user_from_owner(repository.owner)?,
        },
        branch: Branch {
            name: branch_ref.name,
            head: get_commit_info_from_target(head)?,
        },
        errors: resp_data_option.errors.map_or(vec![], |error_list| {
            error_list
                .into_iter()
                .map(|error| GraphqlError {
                    message: error.message,
                })
                .collect::<Vec<GraphqlError>>()
        }),
    })
}

fn get_commit_info_from_target(
    head: branch_head_commit_author::BranchHeadCommitAuthorRepositoryRefTarget,
) -> Result<Commit, FatalError> {
    if let branch_head_commit_author::BranchHeadCommitAuthorRepositoryRefTargetOn::Commit(commit) =
        head.on
    {
        let github_author = commit.author.ok_or(FatalError {
            message: format!("No author on commit {}", commit.oid),
        })?;

        let github_committer = commit.committer.ok_or(FatalError {
            message: format!("No committer on commit {}", commit.oid),
        })?;

        let author = User {
            avatar_url: github_author.avatar_url,
            name: github_author.name,
            handle: github_author.user.map(|user| user.login),
            email: github_author.email,
        };

        let committer = User {
            avatar_url: github_committer.avatar_url,
            name: github_committer.name,
            handle: github_committer.user.map(|user| user.login),
            email: github_committer.email,
        };

        Ok(Commit {
            author,
            committer,
            sha: commit.oid,
        })
    } else {
        Err(FatalError {
            message: format!("ref does not appear to be a commit"),
        })
    }
}

fn get_user_from_owner(
    owner: branch_head_commit_author::BranchHeadCommitAuthorRepositoryOwner,
) -> Result<User, FatalError> {
    match owner.on {
        branch_head_commit_author::BranchHeadCommitAuthorRepositoryOwnerOn::User(user) => {
            Ok(User {
                avatar_url: owner.avatar_url,
                name: user.name,
                email: Option::Some(user.email),
                handle: Option::Some(owner.login),
            })
        }
        _ => Err(FatalError {
            message: "Can only get owner on User, not Organisation.".into(),
        }),
    }
}
