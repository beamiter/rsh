#!/bin/bash

# Test .bashrc compatibility improvements - INTERACTIVE MODE
# This tests that aliases and env vars are loaded when rsh starts

TEST_DIR=$(mktemp -d)
TEST_BASHRC="$TEST_DIR/.bashrc"

cat > "$TEST_BASHRC" << 'EOF'
# Environment variable
export TEST_VAR="hello_world"
export MY_PATH="/custom/path"

# Aliases
alias ll='ls -la'
alias grep='grep --color=auto'
alias mytest='echo "test result: "'

# Shell options
shopt -s extglob
shopt -s dotglob

# Function (won't be fully imported yet, but shouldn't break)
function my_func() {
    echo "Hello from function: $1"
}
EOF

echo "=== Setup ==="
echo "Test .bashrc at: $TEST_BASHRC"

RSH_BIN="./target/debug/rsh"

# Helper function to test with rsh in interactive mode via echo
test_rsh_interactive() {
    local cmd="$1"
    local desc="$2"
    echo "Test: $desc"
    echo "  Command: $cmd"

    # Use echo to pipe command to rsh in interactive mode
    # This simulates user input in interactive shell
    HOME="$TEST_DIR" echo "$cmd" | $RSH_BIN 2>/dev/null | grep -v "^rsh>" || echo "  Result: (no output or failed)"
    echo ""
}

echo "=== Testing rsh interactive mode with custom .bashrc ==="
echo ""

# Test 1: Environment variables
test_rsh_interactive "echo \$TEST_VAR" "Environment variable TEST_VAR"
test_rsh_interactive "echo \$MY_PATH" "Environment variable MY_PATH"

# Test 2: Aliases
test_rsh_interactive "type ll" "Check if alias 'll' is registered"
test_rsh_interactive "alias" "List all aliases"

# Test 3: Shell options
test_rsh_interactive "shopt extglob" "Check extglob option"

# Cleanup
rm -rf "$TEST_DIR"

echo "=== Test Complete ==="
