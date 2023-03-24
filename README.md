# Drone

Drone is an API caching layer application for the Hive blockchain. It is built using Rust with Actix Web, and its primary purpose is to cache and serve API requests for a specific set of methods. While Drone is not meant to be a Jussi replacement, it aims to improve API node performance.

## Features

* Written in Rust for optimal performance and reliability.
* Actix Web for high-performance, asynchronous HTTP handling.
* LRU cache with time-based expiration to store API responses.
* Multiple API endpoints support for seamless request handling with HAF apps.
* Caching support for select Hive API methods to reduce strain on API nodes.


## Cached API Methods

Due to the speed of the blockchain and ease of access, only certain API methods are available for caching by default. This is editable by the node operator if they see the need for it.

* block_api.get_block
* condenser_api.get_block
* account_history_api.get_transaction
* condenser_api.get_transaction
* condenser_api.get_ops_in_block
* condenser_api.get_block_range
* block_api.get_block_range


## Endpoints


The application has the following two primary endpoints:

`GET /`: Health check endpoint that returns the application status, version, and operator message in JSON format.
`POST /`: API call endpoint that takes the JSON-RPC request, caches the response (if supported), and returns the response data.


## Configuration

Drone comes with pre-determined settings, however, you will have to edit ENDPOINT settings in config.json before starting the application (or building the Docker image)

```
PORT: The port on which the application will listen for incoming connections (default: 8999).
HOSTNAME: The hostname/IP address the application will bind to (default: "0.0.0.0").
CACHE_TTL: Time-to-live for cache entries (default: 300 seconds).
CACHE_COUNT: Maximum number of entries the cache can hold (default: 250).
OPERATOR_MESSAGE: Customizable message from the operator (default: "Drone by Deathwing").
HAF_ENDPOINT: HAF Endpoint that Drone can connect to relay HAF related API calls.
HAFAH_ENDPOINT: HAFAH Endpoint that Drone can connect to relay HAFAH related API calls.
HIVEMIND_ENDPOINT: Hivemind Endpoint that Drone can connect to relay Hivemind related API calls.
ACTIX_CONNECTION_THREADS: Specifies the number of HTTP connections kept alive (default: 8).
```

## Usage

### Native

To start the application after altering necessary configuration parameters such as `HAF_ENDPOINT` execute the following command:

`cargo run --release`

If you are advanced and have knowledge about Rust, you can also build the binary using `cargo build --release` and then run it using `./target/release/drone`.

### Docker (Recommended)

You can use docker-compose to build and run Drone.

`docker-compose up --build -d`