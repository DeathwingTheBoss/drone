# Drone

Drone is an API caching layer application for the Hive blockchain. It is built using Rust with Actix Web, and its primary purpose is to cache and serve API requests for a specific set of methods.
Drone is totally meant to be a Jussi replacement, it aims to improve API node performance.

## Features

* Written in Rust for optimal performance and reliability.
* Actix Web for high-performance, asynchronous HTTP handling.
* LRU cache with time-based expiration to store API responses.
* Multiple API endpoints support for seamless request handling with HAF apps.
* Caching support for select Hive API methods to reduce strain on API nodes.


## Cached API Methods

The list of which methods are cached and their cache TTL is configured in the config.yaml file.  The keys used to specify the method names in the config file Jussi's rules for parsing
method names, so you should be able to port your existing Jussi config.json easily.


## Endpoints

The application has the following two primary endpoints:

`GET /`: Health check endpoint that returns the application status, version, and operator message in JSON format.
`POST /`: API call endpoint that takes the JSON-RPC request, caches the response (if supported), and returns the response data.


## Configuration

Drone comes with pre-determined settings, however, you will have to edit ENDPOINT settings in `drone` section of `config.yaml` 
before starting the application (or building the Docker image)

```
port: The port on which the application will listen for incoming connections (default: 8999).
hostname: The hostname/IP address the application will bind to (default: "0.0.0.0").
cache_max_capacity: The approximate max size of the cache, in bytes.  Memory usage may slightly exceed this
                    limit, due to lazy eviction, but not by much.
operator_message: Customizable message from the operator (default: "Drone by Deathwing").
middleware_connection_threads: Specifies the number of HTTP connections to Hive endpoints kept alive (default: 8).
```

## Usage

### Native

To start the application after altering necessary configuration parameters execute the following command:

`cargo run --release`

If you are advanced and have knowledge about Rust, you can also build the binary using `cargo build --release` and then run it using `./target/release/drone`.

### Docker (Recommended)

You can use docker-compose to build and run Drone.

`docker-compose up --build -d`
