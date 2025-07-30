#!/bin/bash

# Test API key error response functionality

echo "Testing API key validation with JSON error responses..."

# Start the server with default API key in background
./target/release/frps --api-key "test123" &
SERVER_PID=$!

# Wait for server to start
sleep 3

echo ""
echo "1. Testing with correct API key (should work, but expect 'No active clients'):"
response=$(curl -s -H "Authorization: Bearer test123" -H "Content-Type: application/json" \
  -d '{"model":"test","messages":[{"role":"user","content":"hello"}]}' \
  http://localhost:18080/v1/chat/completions)
echo "Response: $response"

echo ""
echo "2. Testing with incorrect API key (should return JSON error):"
response=$(curl -s -H "Authorization: Bearer wrong123" -H "Content-Type: application/json" \
  -d '{"model":"test","messages":[{"role":"user","content":"hello"}]}' \
  http://localhost:18080/v1/chat/completions)
echo "Response: $response"

echo ""
echo "3. Testing without API key (should return JSON error):"
response=$(curl -s -H "Content-Type: application/json" \
  -d '{"model":"test","messages":[{"role":"user","content":"hello"}]}' \
  http://localhost:18080/v1/chat/completions)
echo "Response: $response"

echo ""
echo "4. Testing with plain token format (should work):"
response=$(curl -s -H "Authorization: test123" -H "Content-Type: application/json" \
  -d '{"model":"test","messages":[{"role":"user","content":"hello"}]}' \
  http://localhost:18080/v1/chat/completions)
echo "Response: $response"

echo ""
echo "5. Testing with non-HTTP request (should return JSON error):"
response=$(echo "invalid request" | nc localhost 18080)
echo "Response: $response"

# Clean up
kill $SERVER_PID 2>/dev/null
wait $SERVER_PID 2>/dev/null
echo ""
echo "Test completed."