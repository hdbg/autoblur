use anyhow::Result;
use async_trait::async_trait;
use chrono::prelude::*;
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug, Clone)]
pub struct Price {
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
    pub unit: String,
}

#[derive(Deserialize, Debug)]
pub struct CollectionInfo {
    #[serde(rename = "contractAddress")]
    pub contract_address: String,

    pub name: String,

    #[serde(rename = "collectionSlug")]
    pub collection_slug: String,

    #[serde(rename = "imageUrl")]
    pub image_url: Option<String>,

    #[serde(rename = "floorPrice")]
    pub floor_price: Option<Price>,
    #[serde(rename = "floorPriceOneDay")]
    pub floor_price_day: Option<Price>,
    #[serde(rename = "floorPriceOneWeek")]
    pub floor_price_week: Option<Price>,

    #[serde(rename = "volumeFifteenMinutes")]
    pub volume_fifteen_minutes: Option<Price>,
    #[serde(rename = "volumeOneDay")]
    pub volume_day: Option<Price>,
    #[serde(rename = "volumeOneWeek")]
    pub volume_week: Option<Price>,
}

#[derive(Serialize, Deserialize, Debug, Hash, Clone)]
pub struct Bid {
    #[serde(alias = "price")]
    pub value: Decimal,

    #[serde(alias = "executableSize")]
    pub size: u32,

    #[serde(skip)]
    pub timeout: Option<DateTime<Utc>>,
}
impl std::cmp::PartialEq for Bid {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl std::cmp::Eq for Bid {}
impl std::cmp::PartialOrd for Bid {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.value.partial_cmp(&other.value)
    }
}
impl std::cmp::Ord for Bid {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

#[derive(Deserialize, Debug)]
pub struct NotificationSaleData {
    pub to: String,
    pub from: String,

    pub price: Price,
    // SNIP
    #[serde(rename = "imageUrl")]
    pub image_url: String,

    #[serde(rename = "openseaSlug")]
    pub opensea_slug: String,
    #[serde(rename = "collectionName")]
    pub colection_name: String,
    #[serde(rename = "contractAddress")]
    pub contract_address: String,
}

#[derive(Deserialize, Debug)]
pub struct Notification {
    #[serde(rename = "createdAt")]
    pub created_at: chrono::DateTime<Utc>,
    #[serde(rename = "userAddress")]
    pub user_address: String,
    #[serde(rename = "isRead")]
    pub is_read: bool,

    pub data: NotificationSaleData,
}

#[async_trait]
pub trait BlurAPI: Send {
    async fn get_collection_info(&mut self, slug: &str) -> Result<CollectionInfo>;
    async fn get_bids(&mut self, slug: &str) -> Result<Vec<Bid>>;
    async fn get_bids_for_wallet(
        &mut self,
        contract_addr: &str,
        wallet: Option<String>,
    ) -> Result<Vec<Bid>>;
    async fn get_balance_wei(&mut self) -> Result<Decimal>;

    async fn place_bid(&mut self, contract_addr: &str, bid: Bid) -> Result<Bid>;
    async fn cancel_bid(&mut self, contract_addr: &str, bid: Bid) -> Result<()>;
    async fn get_notifications(&mut self) -> Result<Vec<Notification>>;

    async fn get_collections(
        &mut self,
        cursor: &Option<(String, Decimal)>,
    ) -> Result<Vec<CollectionInfo>>;
    async fn iter_collections(
        &mut self,
        cursor: &mut Option<(String, Decimal)>,
    ) -> Result<Vec<CollectionInfo>>;
}
