#!/bin/bash

# test_frpx.sh
# Automated test script for the frpx reverse proxy.

# --- Configuration ---
TEST_CONTROL_PORT=17000
TEST_PROXY_PORT=17001
TEST_PUBLIC_PORT=18080
TEST_LOCAL_PORT=11434

FRPS_LOG="frps.log"
FRPC_A_LOG="frpc_a.log"
FRPC_B_LOG="frpc_b.log"
HTTP_SERVER_LOG="http_server.log"
REQUEST_COUNT=10
WAIT_TIMEOUT=10 # seconds

# --- PID Storage ---
HTTP_PID=""
FRPS_PID=""
FRPC_A_PID=""
FRPC_B_PID=""

# --- Cleanup Function ---
cleanup() {
    echo "
[INFO] Cleaning up background processes..."
    if [ -n "$HTTP_PID" ]; then kill $HTTP_PID &> /dev/null; fi
    if [ -n "$FRPS_PID" ]; then kill $FRPS_PID &> /dev/null; fi
    if [ -n "$FRPC_A_PID" ]; then kill $FRPC_A_PID &> /dev/null; fi
    if [ -n "$FRPC_B_PID" ]; then kill $FRPC_B_PID &> /dev/null; fi
    # Only remove log files if the test passed
    if [ $? -eq 0 ]; then 
        rm -f $FRPS_LOG $FRPC_A_LOG $FRPC_B_LOG $HTTP_SERVER_LOG token.json
        echo "[INFO] Cleanup complete."
    else
        echo "[INFO] Keeping log files for debugging. Cleanup incomplete."
    fi
}

# --- Helper Functions ---
wait_for_log() {
    local log_file=$1
    local pattern=$2
    local timeout=$3
    local start_time=$(date +%s)

    echo "- Waiting for pattern in ${log_file}: \"${pattern}\"
"
    while true; do
        if grep -q "${pattern}" "${log_file}"; then
            echo "  Pattern found."
            return 0
        fi

        local current_time=$(date +%s)
        local elapsed=$((current_time - start_time))
        if [ "$elapsed" -ge "$timeout" ]; then
            echo "[FAIL] Timed out waiting for pattern: \"${pattern}\"
"
            return 1
        fi
        sleep 0.5
    done
}

# --- Pre-Test Cleanup ---
echo "[INFO] Performing pre-test cleanup..."
pkill -f "target/release/frps_demo" &> /dev/null
pkill -f "target/release/frpc_demo" &> /dev/null
pkill -f "python3 -m http.server" &> /dev/null
rm -f token.json  # Remove any existing token file
sleep 1

# Trap EXIT signal to ensure cleanup runs
trap cleanup EXIT

# --- Test Execution ---

echo "
[STEP 1] Compiling project..."
cargo build --release
if [ $? -ne 0 ]; then echo "[FAIL] Cargo build failed."; exit 1; fi
echo "[SUCCESS] Project compiled."


echo "
[STEP 2] Starting background services..."
python3 -m http.server ${TEST_LOCAL_PORT} &> $HTTP_SERVER_LOG &
HTTP_PID=$!
echo "- Local HTTP server started (PID: $HTTP_PID)"

./target/release/frps_demo --control-port ${TEST_CONTROL_PORT} --proxy-port ${TEST_PROXY_PORT} --public-port ${TEST_PUBLIC_PORT} &> $FRPS_LOG &
FRPS_PID=$!
echo "- frps_demo server started (PID: $FRPS_PID)"

wait_for_log $FRPS_LOG "FRPS listening on ports" $WAIT_TIMEOUT || exit 1

./target/release/frpc_demo --client-id client_A --control-port ${TEST_CONTROL_PORT} --proxy-port ${TEST_PROXY_PORT} --local-port ${TEST_LOCAL_PORT} --email test@example.com --password 123456 &> $FRPC_A_LOG &
FRPC_A_PID=$!
echo "- frpc_demo client_A started (PID: $FRPC_A_PID)"

./target/release/frpc_demo --client-id client_B --control-port ${TEST_CONTROL_PORT} --proxy-port ${TEST_PROXY_PORT} --local-port ${TEST_LOCAL_PORT} --email test@example.com --password 123456 &> $FRPC_B_LOG &
FRPC_B_PID=$!
echo "- frpc_demo client_B started (PID: $FRPC_B_PID)"


echo "
[STEP 3] Waiting for clients to register..."
wait_for_log $FRPS_LOG "Client client_A registered successfully" $WAIT_TIMEOUT || exit 1
wait_for_log $FRPS_LOG "Client client_B registered successfully" $WAIT_TIMEOUT || exit 1
echo "[SUCCESS] Both clients registered."


echo "
[STEP 4] Performing ${REQUEST_COUNT} test requests..."
SUCCESSFUL_REQUESTS=0
for i in $(seq 1 $REQUEST_COUNT); do
    HTTP_STATUS=$(curl --max-time 5 -s -o /dev/null -w '%{http_code}' http://localhost:${TEST_PUBLIC_PORT})
    if [ "$HTTP_STATUS" -eq 200 ]; then
        echo "- Request $i: OK (Status: $HTTP_STATUS)"
        SUCCESSFUL_REQUESTS=$((SUCCESSFUL_REQUESTS + 1))
    else
        echo "- Request $i: FAIL (Status: $HTTP_STATUS)"
    fi
    sleep 0.1
done


echo "
[STEP 5] Verifying results..."
if [ "$SUCCESSFUL_REQUESTS" -ne "$REQUEST_COUNT" ]; then
    echo "[FAIL] Not all HTTP requests were successful. Expected ${REQUEST_COUNT}, got ${SUCCESSFUL_REQUESTS}."
    echo "--- frps log ---"; cat $FRPS_LOG; echo "----------------"
    exit 1
else
    echo "[OK] All ${REQUEST_COUNT} HTTP requests returned status 200."
fi

CLIENT_A_HITS=$(grep "Chose client 'client_A'" $FRPS_LOG | wc -l)
CLIENT_B_HITS=$(grep "Chose client 'client_B'" $FRPS_LOG | wc -l)
echo "- Client A handled: $CLIENT_A_HITS requests."
echo "- Client B handled: $CLIENT_B_HITS requests."

if [ "$CLIENT_A_HITS" -gt 0 ] && [ "$CLIENT_B_HITS" -gt 0 ]; then
    echo "[OK] Load balancing is working. Both clients were used."
else
    echo "[FAIL] Load balancing test failed. Not all clients were used."
    echo "--- frps log ---"; cat $FRPS_LOG; echo "----------------"
    exit 1
fi

echo "
--------------------"
echo "[SUCCESS] All tests passed!"
echo "--------------------"

exit 0