#!/bin/bash
# scripts/run_job.sh

CMD=$1
PROJECT_PATH=$2

# Random sleep between 1 and 50 seconds
SLEEP_TIME=$((RANDOM % 60 + 1))
sleep $SLEEP_TIME

cd "$PROJECT_PATH" || exit 1
./target/release/postman "$CMD"