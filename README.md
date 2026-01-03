# NYC Transit HTTP Producer for RustMQ

Real-time NYC subway position data producer, polling MTA GTFS-RT API and streaming to RustMQ message queue.

## Overview

This Rust application polls the MTA GTFS-RT API every 15 seconds and streams live subway vehicle positions to RustMQ using a custom TCP+bincode protocol.

## Tracked Feeds

- **gtfs** - All trains
- **gtfs-ace** - A, C, E lines
- **gtfs-bdfm** - B, D, F, M lines
- **gtfs-g** - G line
- **gtfs-jz** - J, Z lines
- **gtfs-nqrw** - N, Q, R, W lines
- **gtfs-l** - L line
- **gtfs-si** - Staten Island Railway

## Architecture

```
MTA GTFS-RT API (HTTP) → Transit Producer (Polling) → RustMQ (TCP 9092)
```

## Protocol

Uses RustMQ's custom protocol:
- Transport: TCP
- Serialization: bincode
- Message: key (vehicle_id) + value (JSON event)

NOT compatible with Kafka clients.

## Deployment

### Current Configuration
- **Replicas:** 1
- **Resources:**
  - CPU: 50m request, 200m limit
  - Memory: 128Mi request, 512Mi limit
- **Namespace:** data-pipeline
- **Poll Interval:** 15 seconds

### RustMQ Connection
- **Host:** rustmq-0.rustmq-headless.data-pipeline.svc.cluster.local
- **Port:** 9092 (TCP)

## Message Format

### Key
```
Vehicle ID (e.g., "1234")
```

### Value
```json
{
  "event_type": "vehicle_position",
  "source": "mta_gtfs_rt_rust",
  "data": {
    "vehicle_id": "1234",
    "route_id": "1",
    "trip_id": "123456",
    "latitude": 40.7580,
    "longitude": -73.9855,
    "timestamp": 1704216645
  },
  "metadata": {
    "producer_timestamp": 1704216645.123,
    "event_time": 1704216645.123,
    "feed_id": "gtfs"
  }
}
```

## Development

### Build Locally
```bash
cargo build --release
```

### Run Locally (requires RustMQ connection)
```bash
RUST_LOG=transit_http_producer=info cargo run
```

### Docker Build
```bash
docker build -t transit-producer:local .
```

## Dependencies

- **tokio:** Async runtime
- **reqwest:** HTTP client
- **prost:** Protobuf parsing (GTFS-RT format)
- **bincode:** RustMQ message serialization
- **serde/serde_json:** Data serialization
- **tracing:** Logging

## Refactoring from Kafka

**Original:** Used rdkafka library  
**Updated:** Direct TCP connection with bincode serialization  

**Changes Made:**
1. Replaced rdkafka with bincode + tokio TCP
2. Implemented `send_to_rustmq()` helper function
3. Updated connection to RustMQ DNS name
4. Changed from concurrent to sequential feed processing (TCP stream not thread-safe)
5. Message format: bincode-serialized with timestamp and headers

## Performance

- **Poll Interval:** 15 seconds
- **Feeds:** 8 feeds per poll
- **Vehicles:** ~300-500 per poll (varies by time of day)
- **Rate:** ~20-30 vehicles/second during poll
- **Idle:** Between polls (most of the time)

## Status

- ✅ Refactored for RustMQ protocol
- ✅ Updated for AMD64 (AWS EC2)
- ✅ Kubernetes manifests ready
- ⏳ Awaiting deployment

## Links

- **RustMQ:** https://github.com/noelmcmichael/talos-rustmq
- **Harbor:** harbor.int-talos-poc.pocketcove.net/library/transit-producer
- **Namespace:** data-pipeline
- **MTA API:** https://api-endpoint.mta.info/Dataservice/mtagtfsfeeds/nyct%2Fgtfs

---

**Status:** Ready to deploy  
**Platform:** Talos Kubernetes v1.31.2  
**Last Updated:** January 2, 2026
