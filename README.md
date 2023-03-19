# Drone

Drone is an API caching layer application for the Hive blockchain. It is built using Rust with Actix Web, and its primary purpose is to cache and serve API requests for a specific set of methods. While Drone is not meant to be a Jussi replacement, it aims to improve API node performance.

## Features

Written in Rust for optimal performance and reliability.
Actix Web for high-performance, asynchronous HTTP handling.
LRU cache with time-based expiration to store API responses.
Multiple API endpoints support for seamless request handling.
Caching support for select Hive API methods.
Health check endpoint to monitor the application status.


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

Drone comes with pre-determined settings, these should be edited before building the app as node operator sees fit.

```
DRONE_VERSION: Application version, fetched from the Cargo package metadata.
DRONE_PORT: The port on which the application will listen for incoming connections (default: 8999).
DRONE_HOST: The hostname/IP address the application will bind to (default: "localhost").
DRONE_CACHE_TTL: Time-to-live for cache entries (default: 60 seconds).
DRONE_CACHE_COUNT: Maximum number of entries the cache can hold (default: 100).
DRONE_OPERATOR_MESSAGE: Customizable message from the operator (default: "Hive API Cluster - Drone by Deathwing").
DRONE_HAF_ENDPOINT_IP: HAF Endpoint that Drone can connect to relay HAF related API calls.
DRONE_HAFAH_ENDPOINT_IP: HAFAH Endpoint that Drone can connect to relay HAFAH related API calls.
DRONE_HIVEMIND_ENDPOINT_IP: Hivemind Endpoint that Drone can connect to relay Hivemind related API calls.
```

## Usage

To start the application, execute the following command:

`cargo run`

This will start the application and bind it to the pre-configured DRONE_HOST and DRONE_PORT. You can then send API requests to the POST / endpoint using a tool like curl or Postman.