/// Telegraph API Client
pub use error::TelegraphError;
pub mod types;
pub const MAX_SINGLE_FILE_SIZE: usize = 5 * 1024 * 1024;

mod error;

use std::{borrow::Cow, sync::Arc};

use reqwest::{
    multipart::{Form, Part},
    Client, Response,
};
use serde::Serialize;

use crate::http_proxy::HttpRequestBuilder;

use self::{
    error::{ApiResult, UploadResult},
    types::{MediaInfo, Node, Page, PageCreate, PageEdit},
};

const TITLE_LENGTH_MAX: usize = 200;

#[derive(Debug, Clone)]
pub struct Telegraph<T, C = Client> {
    // http client
    client: C,
    // access token
    access_token: T,
}

pub trait AccessToken {
    fn token(&self) -> &str;
    fn select_token(&self, _path: &str) -> &str {
        Self::token(self)
    }
}

#[derive(Debug, Clone)]
pub struct SingleAccessToken(pub Arc<String>);

#[derive(Debug, Clone)]
pub struct RandomAccessToken(pub Arc<Vec<String>>);

impl AccessToken for SingleAccessToken {
    fn token(&self) -> &str {
        &self.0
    }
}

impl From<String> for SingleAccessToken {
    fn from(s: String) -> Self {
        Self(Arc::new(s))
    }
}

impl AccessToken for RandomAccessToken {
    fn token(&self) -> &str {
        use rand::prelude::SliceRandom;
        self.0
            .choose(&mut rand::thread_rng())
            .expect("token list must contains at least one element")
    }
}

impl From<String> for RandomAccessToken {
    fn from(s: String) -> Self {
        Self(Arc::new(vec![s]))
    }
}

impl From<Vec<String>> for RandomAccessToken {
    fn from(ts: Vec<String>) -> Self {
        assert!(!ts.is_empty());
        Self(Arc::new(ts))
    }
}

macro_rules! execute {
    ($send: expr) => {
        $send
            .send()
            .await
            .and_then(Response::error_for_status)?
            .json::<ApiResult<_>>()
            .await?
            .into()
    };
}

#[derive(Debug, Clone, PartialEq, Eq, derive_more::From, derive_more::Into)]
pub struct TelegraphToken(Arc<String>);

impl<T> Telegraph<T, Client> {
    pub fn new<AT>(access_token: AT) -> Telegraph<T, Client>
    where
        AT: Into<T>,
    {
        Telegraph {
            client: Client::new(),
            access_token: access_token.into(),
        }
    }
}

impl<T, C> Telegraph<T, C> {
    pub fn with_proxy<P: HttpRequestBuilder + 'static>(self, proxy: P) -> Telegraph<T, P> {
        Telegraph {
            client: proxy,
            access_token: self.access_token,
        }
    }
}

impl<T, C> Telegraph<T, C>
where
    T: AccessToken,
    C: HttpRequestBuilder,
{
    /// Create page.
    pub async fn create_page(&self, page: &PageCreate) -> Result<Page, TelegraphError> {
        #[derive(Serialize)]
        struct PageCreateShadow<'a> {
            /// Title of the page.
            pub title: &'a str,
            /// Content of the page.
            #[serde(with = "serde_with::json::nested")]
            pub content: &'a Vec<Node>,

            /// Optional. Name of the author, displayed below the title.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub author_name: &'a Option<String>,
            /// Optional. Profile link, opened when users click on the author's name below the title.
            /// Can be any link, not necessarily to a Telegram profile or channel.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub author_url: &'a Option<String>,
        }

        #[derive(Serialize)]
        struct PagePostWithToken<'a> {
            access_token: &'a str,
            #[serde(flatten)]
            page: &'a PageCreateShadow<'a>,
        }

        let title = &page.title[..page.title.len().min(TITLE_LENGTH_MAX)];
        let to_post = PagePostWithToken {
            access_token: self.access_token.token(),
            page: &PageCreateShadow {
                title,
                content: &page.content,
                author_name: &page.author_name,
                author_url: &page.author_url,
            },
        };
        execute!(self
            .client
            .post_builder("https://api.telegra.ph/createPage")
            .form(&to_post))
    }

    /// Edit page.
    pub async fn edit_page(&self, page: &PageEdit) -> Result<Page, TelegraphError> {
        #[derive(Serialize)]
        struct PageEditShadow<'a> {
            /// Title of the page.
            pub title: &'a str,
            /// Path to the page.
            pub path: &'a str,
            /// Content of the page.
            #[serde(with = "serde_with::json::nested")]
            pub content: &'a Vec<Node>,

            /// Optional. Name of the author, displayed below the title.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub author_name: &'a Option<String>,
            /// Optional. Profile link, opened when users click on the author's name below the title.
            /// Can be any link, not necessarily to a Telegram profile or channel.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub author_url: &'a Option<String>,
        }

        #[derive(Serialize)]
        struct PageEditWithToken<'a> {
            access_token: &'a str,
            #[serde(flatten)]
            page: &'a PageEditShadow<'a>,
        }

        let title = &page.title[..page.title.len().min(TITLE_LENGTH_MAX)];
        let to_post = PageEditWithToken {
            access_token: self.access_token.select_token(&page.path),
            page: &PageEditShadow {
                title,
                path: &page.path,
                content: &page.content,
                author_name: &page.author_name,
                author_url: &page.author_url,
            },
        };
        execute!(self
            .client
            .post_builder("https://api.telegra.ph/editPage")
            .form(&to_post))
    }

    /// Get page.
    /// path: Path to the Telegraph page (in the format Title-12-31, i.e. everything
    /// that comes after http://telegra.ph/)
    pub async fn get_page(&self, path: &str) -> Result<Page, TelegraphError> {
        #[derive(Serialize)]
        struct PageGet<'a> {
            path: &'a str,
            #[serde(flatten)]
            return_content: Option<bool>,
        }

        let to_post = PageGet {
            path,
            return_content: Some(true),
        };
        execute!(self
            .client
            .post_builder("https://api.telegra.ph/getPage")
            .form(&to_post))
    }

    /// Upload file.
    /// If the result is Ok, it's length must eq to files'.
    pub async fn upload<IT, I>(&self, files: IT) -> Result<Vec<MediaInfo>, TelegraphError>
    where
        IT: IntoIterator<Item = I>,
        I: Into<Cow<'static, [u8]>>,
    {
        use futures::stream::{self, StreamExt};

        // R2 configuration
        // Replace these values with your own Cloudflare R2 credentials
        const R2_CONFIG: R2Config = R2Config {
            // Your Cloudflare account ID
            // Example: "b6589fc6ab0dc82cf12099d1c2d40ab994e8410c"
            account_id: "your_account_id",

            // R2 Access Key ID from R2 dashboard > Manage R2 API Tokens
            // Example: "54aa49281e6e4f678d1dcd3a49228c98"
            access_key_id: "your_access_key_id",

            // R2 Secret Access Key
            // Example: "77e7c1bf5c48e4da27c5070e7a828c56c11cd6ee"
            secret_key: "your_secret_key",

            // Your R2 bucket name
            // Example: "my-images"
            bucket_name: "your_bucket_name",

            // Your CDN domain for the bucket
            // Example: "cdn.yourdomain.com" or "example.r2.dev"
            cdn_domain: "your.cdn.domain",
        };
        
        let endpoint = format!("https://{}.r2.cloudflarestorage.com", R2_CONFIG.account_id);
        
        let r2_config = aws_sdk_s3::Config::builder()
            .endpoint_url(endpoint)
            .region(aws_types::region::Region::new("auto"))
            .credentials_provider(aws_credential_types::Credentials::new(
                R2_CONFIG.access_key_id,
                R2_CONFIG.secret_key,
                None,
                None,
                "r2-credentials",
            ))
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .build();
    
        let r2_client = aws_sdk_s3::Client::from_conf(r2_config);
        
        let results = stream::iter(files)
            .map(|file| {
                let r2_client = r2_client.clone();
                async move {
                    let file_data: Cow<'static, [u8]> = file.into();
                    let file_ext = infer::get(&file_data)
                        .map(|t| t.extension())
                        .unwrap_or("bin");
    
                    // 使用 uuid 作为文件名
                    let key = format!("{}.{}", uuid::Uuid::new_v4(), file_ext);
                    let bytes = bytes::Bytes::from(file_data.into_owned());
                    let body = aws_sdk_s3::primitives::ByteStream::from(bytes);
                    
                    // 使用 S3 的 PUT 操作，它会自动生成 ETag 并处理重复问题
                    match r2_client
                        .put_object()
                        .bucket(R2_CONFIG.bucket_name)
                        .key(&key)
                        .body(body)
                        .content_type(format!("image/{}", file_ext))
                        .send()
                        .await
                    {
                        Ok(_) => Ok(MediaInfo {
                            src: format!("https://{}/{}", R2_CONFIG.cdn_domain, key)
                        }),
                        Err(e) => {
                            log::error!("Failed to upload file {}: {:?}", key, e);
                            Err(TelegraphError::Server)
                        }
                    }
                }
            })
            .buffer_unordered(10)
            .collect::<Vec<Result<MediaInfo, TelegraphError>>>()
            .await;
        
        let successful_uploads: Vec<MediaInfo> = results
            .into_iter()
            .filter_map(|r| r.ok())
            .collect();
        
        if successful_uploads.is_empty() {
            Err(TelegraphError::Server)
        } else {
            Ok(successful_uploads)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::telegraph::{
        types::{Node, PageCreate},
        SingleAccessToken, Telegraph,
    };

    use super::types::{NodeElement, NodeElementAttr, Tag};

    pub const TELEGRAPH_TOKEN: &str =
        "f42d3570f95412b59b08d64450049e4d609b1f2a57657fce6ce8acc908aa";

    #[ignore]
    #[tokio::test]
    async fn demo_create_page() {
        let telegraph = Telegraph::<SingleAccessToken>::new(TELEGRAPH_TOKEN.to_string());
        let page = PageCreate {
            title: "title".to_string(),
            content: vec![Node::Text("test text".to_string())],
            author_name: Some("test_author".to_string()),
            author_url: Some("https://t.co".to_string()),
        };
        let page = telegraph.create_page(&page).await.unwrap();
        println!("test page: {page:?}");
    }

    #[ignore]
    #[tokio::test]
    async fn demo_upload() {
        let demo_image: Vec<u8> = reqwest::get("https://t.co/static/images/bird.png")
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .as_ref()
            .to_owned();

        let telegraph = Telegraph::<SingleAccessToken>::new(TELEGRAPH_TOKEN.to_string());
        let ret = telegraph
            .upload(Some(demo_image))
            .await
            .unwrap()
            .pop()
            .unwrap();
        println!("uploaded file link: {}", ret.src);
    }

    #[ignore]
    #[tokio::test]
    async fn demo_create_images_page() {
        let telegraph = Telegraph::<SingleAccessToken>::new(TELEGRAPH_TOKEN.to_string());
        let node = Node::NodeElement(NodeElement {
            tag: Tag::Img,
            attrs: Some(NodeElementAttr {
                src: Some("https://telegra.ph/file/e31b40e99b0c028601ccb.png".to_string()),
                href: None,
            }),
            children: None,
        });
        let page = PageCreate {
            title: "title".to_string(),
            content: vec![node],
            author_name: Some("test_author".to_string()),
            author_url: None,
        };
        let page = telegraph.create_page(&page).await.unwrap();
        println!("test page: {page:?}");
    }
}
