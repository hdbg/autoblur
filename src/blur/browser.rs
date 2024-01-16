use std::error::Error;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::emulation::{UserAgentBrandVersion, UserAgentMetadata};
use chromiumoxide::cdp::browser_protocol::network::{self, SetUserAgentOverrideParams};
use chromiumoxide::error::CdpError;
use chromiumoxide::fetcher::{BrowserFetcher, BrowserFetcherOptions};
use chromiumoxide::Page;
use futures::StreamExt;
use tokio;

use regex::Regex;
use tracing::{instrument, trace};

use super::errors::BlurInitError;

const BROWSER_DIR: &str = "autoblur-driver/";
const BROWSER_UDATA_DIR: &str = "autoblur-browser-data/";

#[derive(Debug)]
pub struct BrowserManager {
    b: Browser,
    _task: tokio::task::JoinHandle<()>,
}

impl BrowserManager {
    pub async fn download_browser() -> Result<BrowserConfig, Box<dyn std::error::Error>> {
        let mut download_path = std::env::temp_dir().clone();
        download_path.push(BROWSER_DIR);

        tokio::fs::create_dir_all(&download_path).await?;
        let fetcher = BrowserFetcher::new(
            BrowserFetcherOptions::builder()
                .with_path(&download_path)
                .build()?,
        );
        let info = fetcher.fetch().await?;

        let mut user_dir_path = std::env::temp_dir().clone();
        user_dir_path.push(BROWSER_UDATA_DIR);

        // remove data if already exists
        match tokio::fs::try_exists(user_dir_path.clone()).await {
            Ok(true) => {
                tokio::fs::remove_dir_all(user_dir_path.clone()).await?;
                tokio::fs::create_dir_all(user_dir_path.clone()).await?;
            }
            _ => {
                tokio::fs::create_dir_all(user_dir_path.clone()).await?;
            }
        };

        let config = BrowserConfig::builder()
            .chrome_executable(info.executable_path)
            // .with_head()
            .disable_request_intercept()
            .disable_cache()
            .incognito()
            .arg("--headless=new")
            .arg("--remote-debugging-port=12345")
            .arg("--auto-open-devtools-for-tabs")
            .arg("--disable-web-security") // ignore cors
            .user_data_dir(user_dir_path)
            .build()?;

        Ok(config)
    }

    pub async fn new() -> BrowserManager {
        let browser_config = Self::download_browser()
            .await
            .expect("Browser must be downloaded");

        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .expect("Browser must be launched");

        let task = tokio::task::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    panic!("{}", h.unwrap_err().to_string());
                }
            }
        });

        BrowserManager {
            b: browser,
            _task: task,
        }
    }

    async fn fix_user_agent(&mut self, page: &mut Page) -> Result<(), Box<dyn Error>> {
        tokio::time::sleep(core::time::Duration::from_millis(1000)).await;

        page.execute(network::EnableParams::builder().build())
            .await?;

        let mut ua = page
            .user_agent()
            .await?
            .replace("HeadlessChrome/", "Chrome/");

        if ua.contains("Linux") && !ua.contains("Android") {
            ua = Regex::new(r"\(([^)]+)\)")?
                .replace(&ua, "(Windows NT 10.0; Win64; x64)")
                .to_string();
        };

        let ua_version = if ua.contains("Chrome/") {
            Regex::new(r"Chrome\/([\d|.]+)")?
                .captures(&ua)
                .ok_or(BlurInitError::General)?
                .get(1)
                .ok_or(BlurInitError::General)?
                .as_str()
                .to_string()
        } else {
            let ver = self.b.version().await?.js_version;
            Regex::new(r"\/([\d|.]+)")?
                .captures(&ver)
                .ok_or(BlurInitError::General)?
                .get(1)
                .ok_or(BlurInitError::General)?
                .as_str()
                .to_string()
        };

        let get_platform = |extended: bool| -> &str {
            if ua.contains("Mac OS X") {
                if extended {
                    "Mac OS X"
                } else {
                    "MacIntel"
                }
            } else if ua.contains("Android") {
                "Android"
            } else if ua.contains("Linux") {
                "Linux"
            } else {
                if extended {
                    "Windows"
                } else {
                    "Win32"
                }
            }
        };

        let get_brands = || -> Result<Vec<UserAgentBrandVersion>, Box<dyn Error>> {
            let seed = usize::from_str_radix(
                ua_version.split('.').next().ok_or(BlurInitError::General)?,
                10,
            )?;

            let [x, y, z] = [
                [0, 1, 2],
                [0, 2, 1],
                [1, 0, 2],
                [1, 2, 0],
                [2, 0, 1],
                [2, 1, 0],
            ][seed % 6];

            const ESCAPED_CHARS: [char; 3] = [' ', ' ', ';'];

            let gready_brand = format!(
                "{}Not{}A{}Brand",
                ESCAPED_CHARS[x], ESCAPED_CHARS[y], ESCAPED_CHARS[z]
            )
            .to_string();

            let mut result = Vec::new();

            result.push(UserAgentBrandVersion {
                brand: gready_brand,
                version: "99".to_string(),
            });
            result.push(UserAgentBrandVersion {
                brand: "Chromium".to_owned(),
                version: seed.to_string(),
            });
            result.push(UserAgentBrandVersion {
                brand: "Google Chrome".to_owned(),
                version: seed.to_string(),
            });

            Ok(result)
        };

        let get_platform_version = || -> Result<String, Box<dyn Error>> {
            let re = if ua.contains("Mac OS X ") {
                Regex::new(r"Mac OS X ([^)]+)")?
            } else if ua.contains("Android ") {
                Regex::new(r"Android ([^;]+)")?
            } else if ua.contains("Windows") {
                Regex::new(r"Windows .*?([\d|.]+);?")?
            } else {
                return Ok(String::new());
            };

            Ok(re
                .captures(&ua)
                .ok_or(BlurInitError::General)?
                .get(1)
                .ok_or(BlurInitError::General)?
                .as_str()
                .to_string())
        };

        trace!(new_agent = ua, "browser.evasions.user_agent");

        let ua_override = SetUserAgentOverrideParams {
            user_agent: ua.clone(),
            accept_language: Some("en-US,en".to_owned()),
            platform: Some(get_platform(false).to_owned()),
            user_agent_metadata: Some(UserAgentMetadata {
                brands: Some(get_brands()?),
                full_version_list: Some(vec![UserAgentBrandVersion {
                    brand: "Chrome".to_string(),
                    version: ua_version,
                }]),
                platform: get_platform(true).to_owned(),
                platform_version: get_platform_version()?,
                architecture: "x86".to_string(),
                model: "".to_string(),
                mobile: false,
                bitness: None,
                wow64: None,
            }),
        };

        page.execute(ua_override).await?;

        Ok(())
    }

    #[instrument(skip(self), level = "TRACE")]
    pub async fn new_page(&mut self, url: &str) -> Result<Page, CdpError> {
        let mut res = self.b.new_page(url).await?;
        self.fix_user_agent(&mut res).await.unwrap();

        // res.evaluate_on_new_document(include_str!("scripts/evasions.js"))
        //     .await?;

        Ok(res)
    }
}
