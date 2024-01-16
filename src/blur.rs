use std::{collections::HashSet, env::current_exe};

use self::{
    socket::{Message, Subscription},
    types::{Bid, CollectionInfo},
};

pub mod api;
pub mod browser;
pub mod errors;
mod http;
pub mod socket;
pub mod types;

use anyhow::Result;
use chromiumoxide::cdp::browser_protocol::css::TrackComputedStyleUpdatesParams;
use chrono::{self, DateTime, Duration, Utc};
use futures::StreamExt;
use rust_decimal::Decimal;

use types::BlurAPI;
pub struct MonitoredCollection {
    info: types::CollectionInfo,
    bids: Vec<types::Bid>,
    user_bid: Option<types::Bid>,
}

impl MonitoredCollection {
    fn new(info: CollectionInfo) -> MonitoredCollection {
        Self {
            info,
            bids: vec![],
            user_bid: None,
        }
    }

    fn timed_out_check(&mut self) {
        let bid_timeout_check = |bid: &types::Bid| -> bool {
            match bid.timeout {
                None => true,
                Some(t) => t > chrono::Utc::now(),
            }
        };

        self.bids.retain(bid_timeout_check);
        self.user_bid = self
            .user_bid
            .take()
            .and_then(|b| match bid_timeout_check(&b) {
                true => Some(b),
                false => None,
            });
    }

    pub fn addr(&self) -> &String {
        &self.info.contract_address
    }

    pub fn slug(&self) -> &String {
        &self.info.collection_slug
    }

    pub fn get_bids_mut(&mut self) -> &mut Vec<types::Bid> {
        self.timed_out_check();
        &mut self.bids
    }

    pub fn get_user_bid(&mut self) -> Option<&mut types::Bid> {
        self.timed_out_check();
        self.user_bid.as_mut()
    }
}

const WAIT_DURATION: std::time::Duration = std::time::Duration::from_secs(8);
pub struct ClientOptions {
    pub min_top_bids: u32,
}

pub struct ClientBuilder {
    opts: ClientOptions,
    api: api::TimedAPI<api::API>,
    monitored: Vec<MonitoredCollection>,

    socket_builder: socket::AsyncSocketBuilder,
}
impl ClientBuilder {
    pub async fn new(pk: String, opts: ClientOptions) -> Result<ClientBuilder> {
        let bm = browser::BrowserManager::new().await;
        let api = api::TimedAPI::new(api::API::new(bm, pk).await?, WAIT_DURATION);

        Ok(ClientBuilder {
            api,
            monitored: Vec::new(),
            socket_builder: socket::AsyncSocketBuilder::new(),
            opts,
        })
    }

    pub async fn add_collection(&mut self, slug: &str) -> Result<()> {
        let info = self.api.get_collection_info(slug).await?;

        self.socket_builder
            .subscribe(socket::Subscription::Collection {
                addr: info.contract_address.clone(),
            });

        self.monitored.push(MonitoredCollection::new(info));

        Ok(())
    }

    pub fn build(self) -> Client {
        Client {
            api: self.api,
            socket: self.socket_builder.build(),
            monitored: self.monitored,
            opts: self.opts,
        }
    }
}
pub struct Client {
    api: api::TimedAPI<api::API>,
    socket: socket::AsyncSocket,
    monitored: Vec<MonitoredCollection>,

    opts: ClientOptions,
}

impl Client {
    #[tracing::instrument(skip(api, opts, monitored))]
    async fn bid_checker(
        api: &mut impl types::BlurAPI,
        opts: &ClientOptions,
        monitored: &mut MonitoredCollection,
    ) -> Result<()> {
        monitored.bids.sort();

        let mut appropriate_bid = None;
        let mut current_exec = 0;
        for bid in monitored.bids.iter().rev() {
            current_exec += bid.size;

            if current_exec >= opts.min_top_bids {
                appropriate_bid = Some(bid);
                break;
            }
        }
        let current_balance =
            api.get_balance_wei().await? / Decimal::from_scientific("1e18").unwrap();

        let appropriate_bid = appropriate_bid.and_then(|x| {
            Some(types::Bid {
                value: x.value.min(current_balance),
                ..x.clone()
            })
        });

        if let Some(x) = &appropriate_bid {
            tracing::info!(
                event = "bid.appropriate",
                slug = monitored.slug(),
                appropriate = x.value.to_string()
            );
        }

        let appropriate_is_some = appropriate_bid.is_some();
        let user_bid_is_some = monitored.user_bid.is_some();

        if appropriate_is_some && user_bid_is_some {
            let appropriate_unwrapped = appropriate_bid.unwrap();
            let user_bid = monitored.user_bid.as_ref().unwrap();

            if user_bid.value == appropriate_unwrapped.value {
                tracing::debug!(event = "bid.same", slug = monitored.slug());
                return Ok(());
            }

            let user_bid = monitored.user_bid.take().unwrap();
            tracing::info!(
                event = "bid.cancel",
                coll = monitored.slug(),
                value = user_bid.value.to_string(),
            );
            api.cancel_bid(&monitored.addr(), user_bid).await?;

            let timeout = Utc::now() + Duration::minutes(30);

            let place_value = appropriate_unwrapped.value;
            tracing::info!(
                event = "bid.place",
                coll = monitored.slug(),
                value = place_value.to_string(),
            );

            let bid_to_place = Bid {
                timeout: Some(timeout),
                value: place_value.clone(),
                size: 1,
            };

            api.place_bid(&monitored.addr(), bid_to_place.clone())
                .await?;

            // set new bid
            monitored.user_bid = Some(bid_to_place);
        } else if appropriate_is_some && !user_bid_is_some {
            let appropriate_unwrapped = appropriate_bid.unwrap();
            let timeout = Utc::now() + Duration::minutes(30);

            let place_value = appropriate_unwrapped.value;
            tracing::info!(
                event = "bid.place",
                coll = monitored.slug(),
                value = place_value.to_string(),
            );

            let bid_to_place = Bid {
                timeout: Some(timeout),
                value: place_value.clone(),
                size: 1,
            };

            api.place_bid(&monitored.addr(), bid_to_place.clone())
                .await?;

            // set new bid
            monitored.user_bid = Some(bid_to_place);
        } else if !appropriate_is_some && user_bid_is_some {
            let user_bid = monitored.user_bid.take().unwrap();
            tracing::info!(
                event = "bid.cancel",
                coll = monitored.slug(),
                value = user_bid.value.to_string(),
            );
            api.cancel_bid(&monitored.addr(), user_bid).await?;

            monitored.user_bid = None;
        } else {
            tracing::info!(
                event = "bid.no_update",
                coll = monitored.slug(),
                value = format!("{:?}", monitored.user_bid.clone())
            );
        };

        Ok(())
    }

    async fn initialize_collection(
        api: &mut impl BlurAPI,
        tracked: &mut MonitoredCollection,
    ) -> Result<()> {
        tracing::info!(event = "collection.initializing", slug = tracked.slug());

        let bids = api.get_bids(tracked.slug()).await?;
        tracked.bids.extend(bids.into_iter());

        // delete own previous bids
        let own_bids = api.get_bids_for_wallet(&tracked.addr(), None).await?;

        for bid in own_bids.into_iter() {
            tracing::info!(
                event = "collection.init.bid_cancel",
                slug = tracked.slug(),
                value = format!("{:?}", bid.clone())
            );
            api.cancel_bid(&tracked.addr(), bid).await?;
        }

        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        for tracked in self.monitored.iter_mut() {
            Self::initialize_collection(&mut self.api, tracked).await?;
            Self::bid_checker(&mut self.api, &self.opts, tracked).await?;
        }
        loop {
            let mut stream = self.socket.stream().await?;
            let msg = stream.next().await;
            let msg = msg.unwrap();
            let Message::BidsUpdate {
                contract_addr,
                bids,
            } = msg;
            tracing::info!(
                event = "socket.upd",
                coll = contract_addr,
                bids = format!("{:?}", bids)
            );

            for tracked_coll in self.monitored.iter_mut() {
                if tracked_coll.addr() == &contract_addr {
                    let mut viewed_bids = bids.into_iter().collect::<HashSet<_>>();
                    viewed_bids.retain(|new_bid| {
                        let in_monitored = tracked_coll
                            .get_bids_mut()
                            .iter_mut()
                            .find(|bid| bid.value == new_bid.value);

                        match in_monitored {
                            Some(in_monitored_bid) => {
                                in_monitored_bid.size = new_bid.size;
                                return true;
                            }
                            None => {
                                return true;
                            }
                        }
                    });

                    for remaining_bid in viewed_bids.into_iter() {
                        tracked_coll.get_bids_mut().push(remaining_bid);
                    }

                    Self::bid_checker(&mut self.api, &self.opts, tracked_coll).await?;

                    break;
                }
            }
        }
    }
}
