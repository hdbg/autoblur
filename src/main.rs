use serde::Deserialize;
use tracing::*;
mod blur;

#[derive(Deserialize)]
struct Config {
    pk: String,

    slugs: Vec<String>,

    min_top_bids: u32,
}

#[tokio::main]
async fn main() {
    let config_file = tokio::fs::read_to_string("config.toml").await.unwrap();

    let config: Config = toml::from_str(&config_file).unwrap();
    tracing_subscriber::fmt::init();

    info!("system.init");

    let mut client = blur::ClientBuilder::new(
        config.pk.clone(),
        blur::ClientOptions {
            min_top_bids: config.min_top_bids,
        },
    )
    .await
    .unwrap();

    for slug in config.slugs.iter() {
        client.add_collection(slug).await.unwrap();
    }

    let mut client = client.build();

    client.run().await.unwrap();
}
