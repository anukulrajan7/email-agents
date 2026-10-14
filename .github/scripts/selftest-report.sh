#!/usr/bin/env bash
# Usage: selftest-report.sh <job> <failed step> <log file> <report file>
set -u
job=$1
step=$2
log=$3
out=$4
{
  echo "Job: $job"
  echo "Failed step: $step"
  echo
  echo "Last 80 lines of the log:"
  echo "-----"
  if [ -s "$log" ]; then
    tail -n 80 "$log"
  else
    echo "(no log captured; the job failed before any check produced output)"
  fi
  echo "-----"
} > "$out"
