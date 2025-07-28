# frpx - A Reverse Proxy with Load Balancing

`frpx` is a simple reverse proxy tool, inspired by `frp`, designed to expose local network services to the internet. Its key feature is a built-in random load balancing strategy, allowing for high availability and horizontal scaling of backend services.

This project consists of two main components:
- `frps_demo`: The server application that runs on a publicly accessible machine.
- `frpc_demo`: The client application that runs on the machine with the local service you want to expose.

## Features

- **High Availability**: If a client instance or its local service goes down, the server automatically removes it from the pool of active clients, ensuring new requests are routed only to healthy instances.
- **Horizontal Scaling**: To handle increased load, you can simply run more `frpc_demo` instances. They will automatically register with the server and be included in the load balancing pool.
- **Random Load Balancing**: The server randomly selects one of the available clients to handle each incoming public request, distributing the load evenly.
- **Simple Protocol**: Communication between the server and clients is handled via a straightforward JSON-based command protocol over TCP.
- **Authentication**: Clients must authenticate with the server using email/password or a token before registering.

## Architecture

The system operates using three main ports on the server:

- **Control Port (`17000`)**: Clients establish a persistent connection to this port for registration and to receive commands from the server.
- **Proxy Port (`17001`)**: When the server needs a client to handle a public request, it commands the client to establish a new connection to this port for proxying.
- **Public Port (`18080`)**: This is the public-facing port where end-users connect. The server accepts connections here and forwards them to a chosen client.

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (latest stable version)
- A local web service to test with (e.g., a simple Python server).

### 1. Build the Project

Clone the repository and build the binaries in release mode:

```bash
cargo build --release
```

The compiled binaries will be located in the `target/release/` directory.

### 2. Run a Local Service

For testing, you need a service running on the port that `frpc_demo` will connect to (default is `11434`). You can use the built-in Python web server for this. Open a terminal and run:

```bash
python3 -m http.server 11434
```

### 3. Start the Server (`frps_demo`)

In a new terminal, start the `frps_demo` server:

```bash
cargo run --release --bin frps_demo
```

You should see a log message indicating that the server is listening on all three ports.

When starting the server for the first time, you'll need to authenticate with credentials:
- Email: `test@example.com`
- Password: `123456`

### 4. Start Multiple Clients (`frpc_demo`)

To see the load balancing in action, you need to start at least two client instances. Each client **must have a unique `--client-id`**.

On first run, each client will prompt for authentication credentials:
- Email: `test@example.com`
- Password: `123456`

After successful authentication, a token will be saved to `token.json` for future use.

**Terminal 1 - Client A:**
```bash
cargo run --release --bin frpc_demo -- --client-id client_A
```

**Terminal 2 - Client B:**
```bash
cargo run --release --bin frpc_demo -- --client-id client_B
```

Check the server logs to confirm that both clients have successfully registered.

### 5. Test the Load Balancing

Now you can send requests to the server's public port (`8080`). The server will forward these requests to one of your clients at random.

Use `curl` to make several requests:

```bash
curl http://localhost:8080
curl http://localhost:8080
curl http://localhost:8080
```

Observe the logs in the `frps_demo` terminal. You will see messages like `Chose client 'client_A' for the new connection.` or `Chose client 'client_B' for the new connection.`, demonstrating the random distribution of requests.

## Client Configuration

The `frpc_demo` client can be configured via command-line arguments:

```
Usage: frpc_demo [OPTIONS] --client-id <CLIENT_ID>

Options:
  -c, --client-id <CLIENT_ID>
          Unique ID for this client instance.
  -s, --server-addr <SERVER_ADDR>
          Address of the frps server.
          [default: 127.0.0.1]
      --control-port <CONTROL_PORT>
          Port for the frps control connection.
          [default: 17000]
      --proxy-port <PROXY_PORT>
          Port for the frps proxy connection.
          [default: 17001]
      --local-addr <LOCAL_ADDR>
          Address of the local service to expose.
          [default: 127.0.0.1]
      --local-port <LOCAL_PORT>
          Port of the local service to expose.
          [default: 11434]
  -h, --help
          Print help
  -V, --version
          Print version
```
