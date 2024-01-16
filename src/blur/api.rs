use super::browser::BrowserManager;
use super::{errors::*, http::*, types::*};
use anyhow::{Context, Error, Result};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::{element::Element, Page};
use chrono::prelude::*;
use ethers::types::transaction::eip712::{EIP712Domain, Eip712DomainType, TypedData};
use ethers::{prelude::*, utils::to_checksum};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tracing::instrument;
use tracing::*;

const MAX_FIND_ELEM_ATTEMPTS: i32 = 10;
const FIND_ELEM_WAIT_INTERVAL: u64 = 2000;

const REQ_INTERCEPT_SCRIPT: &'static str = include_str!("scripts/fakemask.js");

const FIRST_BUTT_SELECTOR: &str = ".blur-c-gzBpsk > button:nth-child(1)";
const SECOND_BUTT_SELECTOR: &str =
    "#__next > div > header > div:nth-child(1) > div:nth-child(3) > button > div.Text__TextRoot-sc-m23s7f-0.fsPuzl";
const THIRD_BUTT_SELECTOR: &str = "#METAMASK";

const BLUR_API_BASE: &str = "https://core-api.prod.blur.io/v1";

#[instrument(level = "info")]
async fn safe_wait_for_elem(page: &Page, selector: &str) -> Result<Element> {
    for i in 0..=MAX_FIND_ELEM_ATTEMPTS {
        // page.save
        // let mut path = std::env::current_dir()?;
        // path.push("./test.png");
        // page.save_screenshot(ScreenshotParams::builder().full_page(true).build(), path)
        //     .await?;

        let found_elem = page.find_element(selector).await;
        if let Ok(e) = found_elem {
            return Ok(e);
        // last iteration
        } else if i == MAX_FIND_ELEM_ATTEMPTS {
            Err(BlurInitError::InvalidSelector)?;
        }

        tokio::time::sleep(Duration::from_millis(FIND_ELEM_WAIT_INTERVAL)).await;
    }

    // alas code would never reach here
    Err(anyhow::Error::from(Box::new(BlurInitError::General)))
}

#[derive(Debug)]
pub struct API {
    browser: BrowserManager,
    page: Option<Page>,
    wallet: LocalWallet,
}

impl API {
    #[instrument(skip(self, pk), level = "INFO")]
    async fn auth(&mut self, pk: String) -> Result<()> {
        self.page = Some(
            self.browser
                .new_page("about:blank")
                .await
                .map_err(move |error| Box::new(BlurInitError::BrowserFail(error)))
                .unwrap(),
        );

        let page = self.page.as_mut().unwrap();

        // load interceptor
        // page.evaluate_on_new_document(include_str!("stealth.min.js"))
        //     .await
        //     .map_err(move |error| Box::new(BlurInitError::BrowserFail(error)))?;
        page.evaluate_on_new_document(REQ_INTERCEPT_SCRIPT.to_string().replace("{pk}", &pk))
            .await
            .map_err(move |error| Box::new(BlurInitError::BrowserFail(error)))?;

        page.goto("https://blur.io").await?;

        page.wait_for_navigation().await?;

        tokio::time::sleep(Duration::from_millis(2000)).await;

        for butt in [
            FIRST_BUTT_SELECTOR,
            SECOND_BUTT_SELECTOR,
            THIRD_BUTT_SELECTOR,
        ] {
            let selected = safe_wait_for_elem(page, butt).await?;
            selected.click().await?;

            tokio::time::sleep(Duration::from_millis(2000)).await;
        }

        let target_addr = self.wallet.address();
        let target_addr = to_checksum(&target_addr, None);

        let addr_selector = format!("img[alt=\"{target_addr}\"]");

        safe_wait_for_elem(page, addr_selector.as_str())
            .await
            .map_err(|_x| Box::new(BlurInitError::AuthNotVerified))?;

        info!("auth.succeeded");

        Ok(())
    }

    fn init_wallet(pk: &String) -> Result<LocalWallet> {
        let res = pk
            .parse::<LocalWallet>()
            .map_err(move |x| BlurInitError::InvalidKey)?;

        Ok(res)
    }

    pub async fn new(b: BrowserManager, pk: String) -> Result<API> {
        let mut result = API {
            browser: b,
            page: None,
            wallet: Self::init_wallet(&pk)?,
        };

        result.auth(pk).await?;

        Ok(result)
    }

    async fn common_request(&mut self, req: Request, key_name: &str) -> Result<Value> {
        let page = self.page.as_mut().unwrap();

        let resp = page.send(req).await?;
        let resp_body = resp.body.ok_or(BlurError::NoResponse)?;

        let mut json: Value = serde_json::from_str(&resp_body)
            .map_err(|_x| anyhow::Error::new(BlurError::ParsingError))
            .context("Response is not a json")?;
        let json = json.as_object_mut().ok_or(BlurError::ParsingError)?;

        if !json.contains_key("success") {
            return Err(match resp.status {
                400 => BlurError::CollectionNotFound.into(),
                _ => BlurError::GeneralError.into(),
            });
            return Err(match resp.status {
                400 => BlurError::CollectionNotFound,
                _ => BlurError::GeneralError,
            }
            .into());
        }

        Ok(json
            .remove(key_name)
            .ok_or(anyhow::Error::new(BlurError::ParsingError))
            .with_context(|| format!("Can't get key {key_name}"))?)
    }

    #[instrument(skip(self), level = "trace")]
    pub async fn get_collection_info(&mut self, slug: &str) -> Result<CollectionInfo> {
        let req = Request::builder(format!("{BLUR_API_BASE}/collections/{slug}").as_str())
            .with_method(RequestMethod::Get)
            .build();

        let coll_json = self.common_request(req, "collection").await?;

        let c: CollectionInfo = serde_json::from_value(coll_json).map_err(move |x| Box::new(x))?;
        Ok(c)
    }

    #[instrument(skip(self), level = "trace")]
    pub async fn get_bids(&mut self, slug: &str) -> Result<Vec<Bid>> {
        let page = self.page.as_mut().unwrap();

        let req = Request::builder(
            format!("{BLUR_API_BASE}/collections/{slug}/executable-bids?filters=%7B%7D").as_str(),
        )
        .with_method(RequestMethod::Get)
        .build();

        let bids_json = self.common_request(req, "priceLevels").await?;

        Ok(serde_json::from_value(bids_json)?)
    }

    #[instrument(skip(self), level = "trace")]
    pub async fn get_bids_for_wallet(
        &mut self,
        contract_addr: &str,
        wallet: Option<String>,
    ) -> Result<Vec<Bid>> {
        let wallet = wallet.unwrap_or(self.wallet.address().to_string());

        let req = Request::builder(format!("{BLUR_API_BASE}/collection-bids/user/{wallet}?filters=%7B%22contractAddress%22%3A%22{contract_addr}%22%7D").as_str())
            .with_method(RequestMethod::Get)
            .build();

        let bids_json = self.common_request(req, "priceLevels").await?;

        Ok(serde_json::from_value(bids_json)?)
    }

    fn get_sign_obj(val: &mut Value) -> Result<serde_json::Map<String, Value>> {
        let signatures = val.as_object_mut().ok_or(BlurError::ParsingError)?;
        let signatures = signatures
            .get_mut("signatures")
            .ok_or(BlurError::ParsingError)?;

        let signatures = signatures.as_array_mut().ok_or(BlurError::ParsingError)?;

        let signature_data = signatures[0]
            .as_object_mut()
            .ok_or(BlurError::ParsingError)?;

        let mut signature = signature_data
            .remove("signData")
            .ok_or(BlurError::ParsingError)?;

        Ok(signature
            .as_object_mut()
            .ok_or(BlurError::ParsingError)?
            .clone())
    }

    #[instrument(skip(self), level = "info")]
    pub async fn get_balance_wei(&mut self) -> Result<Decimal> {
        let page = self.page.as_mut().unwrap();
        let exec_result = page
            .evaluate_expression(EvaluateParams {
                await_promise: Some(true),
                return_by_value: Some(true),
                ..EvaluateParams::from("window.autoblur.get_balance()".to_owned())
            })
            .await?;

        let str_val = exec_result
            .value()
            .expect("wallet balance not present")
            .as_str()
            .unwrap()
            .to_owned();

        let mut normalized_decimal = str_val.to_owned().replace("0x", "");
        let trimmed_decimal = normalized_decimal.trim_start_matches("0");

        Decimal::from_str_radix(&trimmed_decimal, 16).map_err(|x| anyhow::Error::from(x))
    }

    fn get_market_place_data(val: &Value) -> Result<&str> {
        let signatures = val.as_object().ok_or(BlurError::ParsingError)?;
        let signatures = signatures
            .get("signatures")
            .ok_or(BlurError::ParsingError)?;

        let signatures = signatures.as_array().ok_or(BlurError::ParsingError)?;

        let signature_data = signatures[0].as_object().ok_or(BlurError::ParsingError)?;

        Ok(signature_data
            .get("marketplaceData")
            .ok_or(BlurError::ParsingError)?
            .as_str()
            .ok_or(BlurError::ParsingError)?)
    }

    fn remove_leading_zeros(
        types: &BTreeMap<String, Vec<Eip712DomainType>>,
        msg: &mut BTreeMap<String, Value>,
    ) -> Result<()> {
        let fix_str = |target: &mut String| {
            if !target.starts_with("0x") {
                return;
            }

            let working_target = target.replace("0x", ""); // remove prefix for work
            let working_target = {
                let mut encountered_non_zero = false;
                let res = working_target
                    .chars()
                    .filter(|x| {
                        if *x != '0' && encountered_non_zero == false {
                            encountered_non_zero = true;
                        };
                        let res = (*x != '0') || encountered_non_zero;

                        res
                    })
                    .collect::<String>();

                res
            };
            let working_target = if working_target.len() == 0 {
                "0".to_owned()
            } else {
                working_target
            };

            *target = String::from("0x") + working_target.as_str();
        };

        if let Some(nonce) = msg.get_mut("nonce") {
            let Some(Value::String(hex_specified)) = nonce.as_object_mut().unwrap().get_mut("hex")
            else {
                panic! {"nonce does not has hex"}
            };
            fix_str(hex_specified);
            let hex_specified = hex_specified.clone();
            *nonce = Value::String(hex_specified);
        }

        for global_type in types.iter() {
            for data_type in global_type.1 {
                let message_val = msg.get_mut(&data_type.name);

                if message_val.is_none() {
                    continue;
                }
                let message_val = message_val.unwrap();

                if data_type.r#type.starts_with("uint") {
                    if let Value::String(val) = message_val {
                        fix_str(val);
                    }
                }
            }
        }

        Ok(())
    }

    #[instrument(skip(self), level = "trace")]
    pub async fn place_bid(&mut self, contract_addr: &str, mut bid: Bid) -> Result<Bid> {
        let page = self.page.as_mut().unwrap();

        bid.timeout = bid
            .timeout
            .or(Some(Utc::now() + chrono::Duration::hours(1))); // or 1 hour

        let sign_data_request =
            Request::builder(format!("{BLUR_API_BASE}/collection-bids/format").as_str())
                .with_header("Content-Type", "application/json")
                .with_body(
                    json!({
                        "contractAddress": contract_addr,
                        "expirationTime": bid.timeout.unwrap().to_rfc3339(),
                        "price": {
                            "amount": bid.value.to_string(),
                            "unit": "BETH"
                        },
                        "quantity": bid.size
                    })
                    .to_string(),
                )
                .with_method(RequestMethod::Post)
                .build();

        let sign_data_response = page.send(sign_data_request).await?;
        let mut sign_data_json: Value = serde_json::from_str(
            sign_data_response
                .body
                .ok_or(BlurError::NoResponse)?
                .as_str(),
        )?;

        if sign_data_response.status != 201 {
            let msg = sign_data_json
                .as_object()
                .ok_or(BlurError::ParsingError)?
                .get("message")
                .ok_or(BlurError::ParsingError)?
                .as_str()
                .unwrap();

            let err = match msg {
                "Insufficient funds" => BlurError::NotEnoughBalance,
                _ => BlurError::GeneralError,
            };

            return Err(err.into());
        }

        // let mut sign_data_response: Value = serde_json::from_str(include_str!("fakereq.json"))?;
        let mut signature = Self::get_sign_obj(&mut sign_data_json)?;

        let domain: EIP712Domain =
            serde_json::from_value(signature.remove("domain").ok_or(BlurError::ParsingError)?)?;
        let types: BTreeMap<String, Vec<Eip712DomainType>> =
            serde_json::from_value(signature.remove("types").ok_or(BlurError::ParsingError)?)?;
        let mut message: BTreeMap<String, Value> =
            serde_json::from_value(signature.remove("value").ok_or(BlurError::ParsingError)?)?;

        // fix str uint to real uint
        Self::remove_leading_zeros(&types, &mut message)?;

        let typed_data = TypedData {
            domain,
            types,
            message,
            primary_type: "Order".to_string(),
        };

        let bid_signature = self.wallet.sign_typed_data(&typed_data).await;

        let bid_signature = bid_signature.unwrap().to_string();

        let sumbit_sign_req =
            Request::builder(format!("{BLUR_API_BASE}/collection-bids/submit").as_str())
                .with_method(RequestMethod::Post)
                .with_header("Content-Type", "application/json")
                .with_body(
                    json!({
                        "signature": "0x".to_string() + &bid_signature,
                        "marketplaceData": Self::get_market_place_data(&sign_data_json)?
                    })
                    .to_string(),
                )
                .build();

        let submit_sign_resp = page.send(sumbit_sign_req).await?;

        if submit_sign_resp.status == 201 {
            Ok(bid)
        } else {
            Err(BlurError::GeneralError.into())
        }
    }

    #[instrument(skip(self), level = "trace")]
    pub async fn cancel_bid(&mut self, contract_addr: &str, bid: Bid) -> Result<()> {
        let page = self.page.as_mut().unwrap();

        let req = Request::builder(format!("{BLUR_API_BASE}/collection-bids/cancel").as_str())
            .with_method(RequestMethod::Post)
            .with_header("Content-Type", "application/json")
            .with_body(
                json!({
                    "contractAddress": contract_addr,
                    "prices": [bid.value.to_string()]
                })
                .to_string(),
            )
            .build();

        let resp = page.send(req).await?;
        let resp_json: Value =
            serde_json::from_str(resp.body.ok_or(BlurError::NoResponse)?.as_str())?;

        if resp.status == 201
            && resp_json.get("success").is_some()
            && resp_json
                .get("success")
                .unwrap()
                .as_bool()
                .ok_or(BlurError::ParsingError)?
        {
            return Ok(());
        }

        use BlurError::*;
        if let Some(Value::String(msg)) = resp_json.get("message") {
            return Err(match msg.as_str() {
                "No bids found" => BidNotExists,
                _ => GeneralError,
            }
            .into());
        }

        Err(GeneralError.into())
    }

    #[instrument(skip(self), level = "trace")]
    pub async fn get_notifications(&mut self) -> Result<Vec<Notification>> {
        let req = Request::builder(format!("{BLUR_API_BASE}/user/notifications/feed").as_str())
            .with_method(RequestMethod::Get)
            .build();

        let resp = self.page.as_mut().unwrap().send(req).await?;

        let resp_json: Value = serde_json::from_str(&(resp.body.ok_or(BlurError::NoResponse)?))?;
        let resp_json = resp_json.as_object().ok_or(BlurError::ParsingError)?;

        let is_success = resp_json
            .get("success")
            .ok_or(BlurError::ParsingError)?
            .as_bool()
            .ok_or(BlurError::ParsingError)?;

        let notifications_json = resp_json
            .get("notifications")
            .ok_or(BlurError::ParsingError)?
            .as_array()
            .ok_or(BlurError::ParsingError)?;

        let mut result = Vec::new();
        result.reserve(notifications_json.len());

        for notification in notifications_json {
            let notification = notification.as_object().ok_or(BlurError::GeneralError)?;
            let notification_type = notification
                .get("notificationType")
                .ok_or(BlurError::GeneralError)?;
            let notification_type = notification_type.as_str().ok_or(BlurError::GeneralError)?;

            if notification_type != "sale" {
                continue;
            }

            result.push(serde_json::from_value((notification.clone()).into())?);
        }

        Ok(result)
    }

    pub async fn get_collections(
        &mut self,
        cursor: &Option<(String, Decimal)>,
    ) -> Result<Vec<CollectionInfo>> {
        let mut filters = match cursor.as_ref() {
            Some(x) => {
                json!({"cursor":{"contractAddress":x.0,"volumeOneDay":x.1.to_string()},"sort":"VOLUME_ONE_DAY","order":"DESC"})
            }
            None => json!(	{"sort":"VOLUME_ONE_DAY","order":"DESC"}),
        };

        let url = format!(
            "https://core-api.prod.blur.io/v1/collections/?filters={}",
            filters
        )
        .to_owned();

        let req = Request::builder(&url)
            .with_method(RequestMethod::Get)
            .build();

        let collections_json = self.common_request(req, "collections").await?;

        let collections_json = collections_json.as_array().ok_or(BlurError::NoResponse)?;
        let mut result = Vec::new();
        for item in collections_json {
            let parsed: CollectionInfo = serde_json::from_value(dbg!(item).clone())?;
            result.push(parsed);
        }

        Ok(result)
    }

    pub async fn iter_collections(
        &mut self,
        cursor: &mut Option<(String, Decimal)>,
    ) -> Result<Vec<CollectionInfo>> {
        let res = self.get_collections(cursor).await?;
        let last_item = res.last().ok_or(BlurError::GeneralError)?;

        *cursor = Some((
            last_item.contract_address.clone(),
            last_item.volume_day.clone().unwrap().amount.clone(),
        ));

        Ok(res)
    }
}

#[async_trait::async_trait]
impl BlurAPI for API {
    async fn get_collection_info(&mut self, slug: &str) -> Result<CollectionInfo> {
        self.get_collection_info(slug).await
    }
    async fn get_bids(&mut self, slug: &str) -> Result<Vec<Bid>> {
        self.get_bids(slug).await
    }
    async fn get_bids_for_wallet(
        &mut self,
        contract_addr: &str,
        wallet: Option<String>,
    ) -> Result<Vec<Bid>> {
        self.get_bids_for_wallet(contract_addr, wallet).await
    }
    async fn get_balance_wei(&mut self) -> Result<Decimal> {
        self.get_balance_wei().await
    }

    async fn place_bid(&mut self, contract_addr: &str, bid: Bid) -> Result<Bid> {
        self.place_bid(contract_addr, bid).await
    }
    async fn cancel_bid(&mut self, contract_addr: &str, bid: Bid) -> Result<()> {
        self.cancel_bid(contract_addr, bid).await
    }
    async fn get_notifications(&mut self) -> Result<Vec<Notification>> {
        self.get_notifications().await
    }

    async fn get_collections(
        &mut self,
        cursor: &Option<(String, Decimal)>,
    ) -> Result<Vec<CollectionInfo>> {
        self.get_collections(cursor).await
    }
    async fn iter_collections(
        &mut self,
        cursor: &mut Option<(String, Decimal)>,
    ) -> Result<Vec<CollectionInfo>> {
        self.iter_collections(cursor).await
    }
}

pub struct TimedAPI<T: BlurAPI + Send> {
    api: T,
    last_checked: std::time::Instant,
    delay: std::time::Duration,
}

impl<T: BlurAPI + Send> TimedAPI<T> {
    pub fn new(api: T, delay: std::time::Duration) -> Self {
        Self {
            api,
            delay,
            last_checked: Instant::now() - delay,
        }
    }

    async fn check_and_wait(&mut self) {
        let since_last = Instant::now() - self.last_checked;
        tracing::debug!(event = "api.timed.passed", since = since_last.as_millis());
        if since_last < self.delay {
            let time_diff = self.delay - since_last;
            tokio::time::sleep(time_diff).await;
            tracing::debug!(event = "api.timed.waiting", diff = time_diff.as_millis());
        }
        self.last_checked = Instant::now();
    }
}
#[async_trait::async_trait]
impl<T: BlurAPI + Send> BlurAPI for TimedAPI<T> {
    async fn get_collection_info(&mut self, slug: &str) -> Result<CollectionInfo> {
        self.check_and_wait().await;
        self.api.get_collection_info(slug).await
    }
    async fn get_bids(&mut self, slug: &str) -> Result<Vec<Bid>> {
        self.check_and_wait().await;

        self.api.get_bids(slug).await
    }
    async fn get_bids_for_wallet(
        &mut self,
        contract_addr: &str,
        wallet: Option<String>,
    ) -> Result<Vec<Bid>> {
        self.check_and_wait().await;
        self.api.get_bids_for_wallet(contract_addr, wallet).await
    }
    async fn get_balance_wei(&mut self) -> Result<Decimal> {
        self.check_and_wait().await;

        self.api.get_balance_wei().await
    }

    async fn place_bid(&mut self, contract_addr: &str, bid: Bid) -> Result<Bid> {
        self.check_and_wait().await;

        self.api.place_bid(contract_addr, bid).await
    }
    async fn cancel_bid(&mut self, contract_addr: &str, bid: Bid) -> Result<()> {
        self.check_and_wait().await;

        self.api.cancel_bid(contract_addr, bid).await
    }
    async fn get_notifications(&mut self) -> Result<Vec<Notification>> {
        self.check_and_wait().await;

        self.api.get_notifications().await
    }

    async fn get_collections(
        &mut self,
        cursor: &Option<(String, Decimal)>,
    ) -> Result<Vec<CollectionInfo>> {
        self.check_and_wait().await;

        self.api.get_collections(cursor).await
    }
    async fn iter_collections(
        &mut self,
        cursor: &mut Option<(String, Decimal)>,
    ) -> Result<Vec<CollectionInfo>> {
        self.check_and_wait().await;

        self.api.iter_collections(cursor).await
    }
}
