#!/usr/bin/env bash

ip link set wlp1s0 down
sleep $1
ip link set wlp1s0 up
