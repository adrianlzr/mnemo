#!/bin/bash

# System tuning script for maximum UDP performance
# Run with sudo for best results

echo "=== UDP Performance System Tuning ==="

# Network buffer tuning
echo "Tuning network buffers..."
echo 'net.core.rmem_max = 134217728' >> /etc/sysctl.conf
echo 'net.core.rmem_default = 134217728' >> /etc/sysctl.conf
echo 'net.core.wmem_max = 134217728' >> /etc/sysctl.conf
echo 'net.core.wmem_default = 134217728' >> /etc/sysctl.conf
echo 'net.core.netdev_max_backlog = 5000' >> /etc/sysctl.conf
echo 'net.core.netdev_budget = 600' >> /etc/sysctl.conf

# UDP specific tuning
echo 'net.ipv4.udp_mem = 102400 873800 16777216' >> /etc/sysctl.conf
echo 'net.ipv4.udp_rmem_min = 8192' >> /etc/sysctl.conf
echo 'net.ipv4.udp_wmem_min = 8192' >> /etc/sysctl.conf

# CPU and interrupt tuning
echo 'kernel.sched_rt_runtime_us = -1' >> /etc/sysctl.conf
echo 'kernel.sched_rt_period_us = 1000000' >> /etc/sysctl.conf

# Apply settings
sysctl -p

# Set CPU governor to performance mode
echo "Setting CPU governor to performance..."
for cpu in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
    echo performance > $cpu 2>/dev/null || true
done

# Disable CPU frequency scaling
echo "Disabling CPU frequency scaling..."
for cpu in /sys/devices/system/cpu/cpu*/cpufreq/scaling_max_freq; do
    if [ -f $cpu ]; then
        cat /sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq > $cpu 2>/dev/null || true
    fi
done

# Set process priority
echo "Setting process priority..."
echo "To run the server with high priority, use:"
echo "sudo nice -n -20 ./target/release/mnemo"
echo "or"
echo "sudo chrt -f 99 ./target/release/mnemo"

# IRQ affinity (Linux only)
if [ -d /proc/irq ]; then
    echo "Setting IRQ affinity..."
    # Get network interface IRQ
    NET_INTERFACE=$(ip route | grep default | awk '{print $5}' | head -1)
    if [ ! -z "$NET_INTERFACE" ]; then
        IRQ_NUM=$(grep $NET_INTERFACE /proc/interrupts | awk '{print $1}' | sed 's/://')
        if [ ! -z "$IRQ_NUM" ]; then
            echo "Setting IRQ $IRQ_NUM affinity to CPU 0"
            echo 1 > /proc/irq/$IRQ_NUM/smp_affinity 2>/dev/null || true
        fi
    fi
fi

echo "=== Tuning Complete ==="
echo "Reboot recommended for all changes to take effect"
echo ""
echo "To run the server with maximum performance:"
echo "1. Build in release mode: cargo build --release"
echo "2. Set CPU affinity: taskset -c 0-$(nproc) ./target/release/mnemo"
echo "3. Use high priority: sudo nice -n -20 ./target/release/mnemo"
