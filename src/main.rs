use prost::Message as ProstMessage;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{error, info, warn};

// Configuration constants
const RUSTMQ_HOST: &str = "rustmq-0.rustmq-headless.data-pipeline.svc.cluster.local";
const RUSTMQ_PORT: u16 = 9092;
const MTA_API_BASE: &str = "https://api-endpoint.mta.info/Dataservice/mtagtfsfeeds/nyct%2F";
const POLL_INTERVAL_SECS: u64 = 5;

// RustMQ Message structure (must match RustMQ's message.rs)
#[derive(Debug, Serialize, Deserialize)]
struct RustMQMessage {
    key: Vec<u8>,
    value: Vec<u8>,
    timestamp: u64,
    #[serde(default)]
    headers: Vec<(String, String)>,
}

impl RustMQMessage {
    fn new(key: Vec<u8>, value: Vec<u8>) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;
        
        Self {
            key,
            value,
            timestamp,
            headers: Vec::new(),
        }
    }
}

// MTA feed IDs
const FEED_IDS: &[&str] = &[
    "gtfs",       // All trains
    "gtfs-ace",   // A, C, E
    "gtfs-bdfm",  // B, D, F, M
    "gtfs-g",     // G
    "gtfs-jz",    // J, Z
    "gtfs-nqrw",  // N, Q, R, W
    "gtfs-l",     // L
    "gtfs-si",    // Staten Island Railway
];

// Simplified GTFS-RT structures (subset we care about)
#[derive(Clone, PartialEq, prost::Message)]
struct FeedMessage {
    #[prost(message, optional, tag = "1")]
    header: Option<FeedHeader>,
    #[prost(message, repeated, tag = "2")]
    entity: Vec<FeedEntity>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct FeedHeader {
    #[prost(string, optional, tag = "1")]
    gtfs_realtime_version: Option<String>,
    #[prost(uint64, optional, tag = "3")]
    timestamp: Option<u64>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct FeedEntity {
    #[prost(string, optional, tag = "1")]
    id: Option<String>,
    #[prost(message, optional, tag = "2")]
    vehicle: Option<VehiclePosition>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct VehiclePosition {
    #[prost(message, optional, tag = "1")]
    trip: Option<TripDescriptor>,
    #[prost(message, optional, tag = "2")]
    position: Option<Position>,
    #[prost(uint64, optional, tag = "4")]
    timestamp: Option<u64>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct TripDescriptor {
    #[prost(string, optional, tag = "1")]
    trip_id: Option<String>,
    #[prost(string, optional, tag = "5")]
    route_id: Option<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct Position {
    #[prost(float, optional, tag = "1")]
    latitude: Option<f32>,
    #[prost(float, optional, tag = "2")]
    longitude: Option<f32>,
}

// Output event structure
#[derive(Debug, Serialize)]
struct TransitPositionEvent {
    event_type: String,
    source: String,
    data: TransitData,
    metadata: EventMetadata,
}

#[derive(Debug, Serialize)]
struct TransitData {
    vehicle_id: String,
    route_id: Option<String>,
    trip_id: Option<String>,
    latitude: f32,
    longitude: f32,
    timestamp: u64,
}

#[derive(Debug, Serialize)]
struct EventMetadata {
    producer_timestamp: f64,
    event_time: f64,
    feed_id: String,
}

async fn fetch_and_process_feed(
    client: &reqwest::Client,
    rustmq_stream: &mut TcpStream,
    feed_id: &str,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}{}", MTA_API_BASE, feed_id);
    
    // Fetch protobuf data
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await?;

    let bytes = response.bytes().await?;
    
    // Parse protobuf
    let feed_message = FeedMessage::decode(&bytes[..])?;
    
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    
    let mut messages_sent = 0u64;
    
    // Process each entity
    for entity in feed_message.entity {
        // Skip if no ID
        let vehicle_id = match &entity.id {
            Some(id) => id.clone(),
            None => continue,
        };
        
        if let Some(vehicle) = entity.vehicle {
            if let Some(position) = vehicle.position {
                // Skip if no lat/lon
                let latitude = match position.latitude {
                    Some(lat) => lat,
                    None => continue,
                };
                let longitude = match position.longitude {
                    Some(lon) => lon,
                    None => continue,
                };
                
                let route_id = vehicle.trip.as_ref()
                    .and_then(|t| t.route_id.clone());
                let trip_id = vehicle.trip.as_ref()
                    .and_then(|t| t.trip_id.clone());
                let timestamp = vehicle.timestamp.unwrap_or(now as u64);
                
                // Create event
                let event = TransitPositionEvent {
                    event_type: "vehicle_position".to_string(),
                    source: "mta_gtfs_rt_rust".to_string(),
                    data: TransitData {
                        vehicle_id: vehicle_id.clone(),
                        route_id,
                        trip_id,
                        latitude,
                        longitude,
                        timestamp,
                    },
                    metadata: EventMetadata {
                        producer_timestamp: now,
                        event_time: now,
                        feed_id: feed_id.to_string(),
                    },
                };
                
                // Serialize and send to RustMQ
                match serde_json::to_string(&event) {
                    Ok(event_json) => {
                        let key = vehicle_id.as_bytes();
                        let value = event_json.as_bytes();

                        match send_to_rustmq(rustmq_stream, key, value).await {
                            Ok(_) => {
                                messages_sent += 1;
                            }
                            Err(e) => {
                                error!("Failed to send to RustMQ: {:?}", e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to serialize event: {}", e);
                    }
                }
            }
        }
    }
    
    Ok(messages_sent)
}

// RustMQ producer helper function
async fn send_to_rustmq(
    stream: &mut TcpStream,
    key: &[u8],
    value: &[u8],
) -> Result<u64, Box<dyn std::error::Error>> {
    // Create RustMQ message
    let message = RustMQMessage::new(key.to_vec(), value.to_vec());

    // Serialize message using bincode
    let msg_bytes = bincode::serialize(&message)?;
    let msg_len = msg_bytes.len() as u32;

    // Send length (u32, big-endian)
    stream.write_u32(msg_len).await?;

    // Send message bytes
    stream.write_all(&msg_bytes).await?;

    // Read offset response (u64, big-endian)
    let offset = stream.read_u64().await?;

    if offset == u64::MAX {
        return Err("Server returned error (u64::MAX)".into());
    }

    Ok(offset)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("transit_http_producer=info")
        .init();

    info!("🚀 Transit HTTP Producer (Rust) starting...");
    info!("📡 MTA GTFS-RT API: {}", MTA_API_BASE);
    info!("📤 RustMQ host: {}:{}", RUSTMQ_HOST, RUSTMQ_PORT);
    info!("🚇 Polling {} feeds every {}s", FEED_IDS.len(), POLL_INTERVAL_SECS);

    // Create HTTP client
    let client = reqwest::Client::new();

    // Connect to RustMQ
    let rustmq_addr = format!("{}:{}", RUSTMQ_HOST, RUSTMQ_PORT);
    let mut rustmq_stream = TcpStream::connect(&rustmq_addr)
        .await
        .expect("Failed to connect to RustMQ");

    info!("✅ Connected to RustMQ at {}", rustmq_addr);
    info!("⏳ Starting polling loop...\n");

    // Statistics
    let mut total_messages = 0u64;
    let mut poll_count = 0u64;
    let mut errors = 0u64;

    // Main polling loop
    loop {
        let poll_start = std::time::Instant::now();
        let mut batch_messages = 0u64;

        // Fetch all feeds sequentially (TCP stream not thread-safe)
        for feed_id in FEED_IDS {
            match fetch_and_process_feed(&client, &mut rustmq_stream, feed_id).await {
                Ok(count) => {
                    batch_messages += count;
                    info!("✅ {}: {} vehicles", feed_id, count);
                }
                Err(e) => {
                    errors += 1;
                    warn!("⚠️  {}: {}", feed_id, e);
                }
            }
        }

        total_messages += batch_messages;
        poll_count += 1;

        let elapsed = poll_start.elapsed();
        let rate = batch_messages as f64 / elapsed.as_secs_f64();

        info!(
            "\n📊 Poll #{} complete: {} msgs in {:.1}s ({:.1} msg/s)",
            poll_count,
            batch_messages,
            elapsed.as_secs_f64(),
            rate
        );
        info!(
            "📈 Total: {} msgs | Avg: {:.1} msg/poll | Errors: {}\n",
            total_messages,
            total_messages as f64 / poll_count as f64,
            errors
        );

        // Wait before next poll
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
    }
}
