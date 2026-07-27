// Command benchmark is a UDP load generator / latency benchmark for the
// GET/UPDATE key-value wire protocol used throughout this project.
//
// Rewritten to fix three real bugs found in the original:
//
//  1. Data race: every in-flight goroutine read into the SAME response
//     buffer. Fixed by giving each worker goroutine (not each request) its
//     own buffer — see below on why it's per-worker rather than per-request.
//
//  2. Data race: latencies/successful/failures/timeouts were mutated from multiple
//     goroutines with no synchronization (unguarded slice append, plain
//     int++). Fixed by having each worker own its own result slice/counters
//     and hand them off over a channel — no shared mutable state at all.
//
//  3. No request/response correlation: the wire protocol has no request-ID
//     field, so >1 in-flight request per UDP socket is unsafe — a response
//     datagram can't be matched to the request that caused it, and
//     concurrent goroutines calling SetReadDeadline/Read on the same
//     *net.UDPConn race on the connection's shared deadline state regardless
//     of the buffer fix above. Fixed by making each worker a strictly
//     sequential closed-loop client (send, block for the reply, repeat) on
//     its own dedicated connection. All concurrency now comes from running
//     more independent workers (-clients), which is also the only form of
//     concurrency that matches how the Rust/Go/C++ servers are actually
//     exercised by real traffic.
//
// A fourth issue, unrelated to correctness but relevant to measurement
// accuracy: the original generated every worker's requests (including
// UUID + xxh3 hashing) *inside* the timed goroutine, after the benchmark's
// global start time was already recorded. That means elapsed-time-based
// RPS included request-generation cost, not just network round-trip time.
// This version generates every request for every thread up front, before
// starting the clock, so the measured window is pure network I/O.
package main

import (
	"encoding/binary"
	"flag"
	"fmt"
	"math/rand/v2"
	"net"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/zeebo/xxh3"
)

type OP int

const (
	OP_UNKNOWN OP = 0
	OP_GET     OP = 1
	OP_UPDATE  OP = 2
)

func NewOP(op string) OP {
	switch strings.ToLower(op) {
	case "get":
		return OP_GET
	case "update":
		return OP_UPDATE
	default:
		return OP_UNKNOWN
	}
}

const (
	OP_SIZE  = 1
	KEY_SIZE = 8
	VAL_SIZE = 36

	// The wire protocol always sends a fixed-size 45-byte request
	// (OP + KEY + VALUE) regardless of opcode — the server ignores the
	// value bytes on a GET, but still expects them to be present.
	REQUEST_SIZE = OP_SIZE + KEY_SIZE + VAL_SIZE

	// NOTE: this assumes a response of VALUE only (36 bytes), matching the
	// Rust server's wire format. If you're benchmarking the Go/C++ servers
	// from earlier in this conversation, their responses are KEY+VALUE
	// (44 bytes) — on UDP, reading a 44-byte datagram into a 36-byte buffer
	// silently truncates rather than erroring, so latency/success numbers
	// would still look fine while the actual bytes you'd get back are
	// wrong. Double check which server you're pointed at and adjust
	// RESPONSE_SIZE accordingly before trusting a cross-implementation
	// comparison.
	RESPONSE_SIZE = VAL_SIZE

	RESPONSE_TIMEOUT = 10 * time.Millisecond
)

// request is a single pre-generated wire-format datagram, built before the
// benchmark clock starts so that UUID/hash generation cost is never counted
// as part of measured latency or throughput.
type request struct {
	data []byte
	op   OP
}

func randOP() OP {
	// math/rand/v2 instead of crypto/rand: this only needs variety across
	// requests, not unpredictability against an adversary, and crypto/rand
	// is meaningfully more expensive per call (syscall-backed entropy pool)
	// — not that it mattered for correctness, but it was needless cost even
	// back when generation was in the timed path.
	if rand.IntN(2) == 0 {
		return OP_GET
	}
	return OP_UPDATE
}

func randKey() []byte {
	bundleID := uuid.New().String()
	idfa := uuid.New().String()
	hash := xxh3.HashString(bundleID + idfa)

	key := make([]byte, KEY_SIZE)
	binary.BigEndian.PutUint64(key, hash)
	return key
}

func randVal() []byte {
	val := make([]byte, VAL_SIZE)
	copy(val, uuid.New().String())
	return val
}

// buildRequest constructs one fixed-size wire request. The value bytes are
// zero for GET (the server ignores them; keeping the wire size constant is
// what the protocol requires, not what a GET semantically needs).
func buildRequest() request {
	op := randOP()
	data := make([]byte, REQUEST_SIZE)
	data[0] = byte(op)
	copy(data[OP_SIZE:OP_SIZE+KEY_SIZE], randKey())
	if op == OP_UPDATE {
		copy(data[OP_SIZE+KEY_SIZE:], randVal())
	}
	return request{data: data, op: op}
}

// workerResult is what one worker goroutine reports after finishing its
// share of requests. Each worker owns its own instance entirely — nothing
// here is ever touched by more than one goroutine, so no locking is needed
// anywhere in this file.
type workerResult struct {
	latencies                    []float64
	successful, failed, timeouts int64
	gets, updates                int64
}

// runWorker is a strictly sequential closed-loop client: send, wait for the
// matching reply (or time out), record latency, repeat. Exactly one request
// is ever in flight on conn at a time, which is what makes this safe without
// a request-ID field in the wire protocol — there's never ambiguity about
// which datagram is the reply to which request.
func runWorker(conn *net.UDPConn, reqs []request) workerResult {
	result := workerResult{latencies: make([]float64, 0, len(reqs))}
	response := make([]byte, RESPONSE_SIZE) // this worker's own buffer, reused safely across its own sequential requests

	for _, req := range reqs {
		if req.op == OP_UPDATE {
			result.updates++
		} else {
			result.gets++
		}

		start := time.Now()
		if _, err := conn.Write(req.data); err != nil {
			result.failed++
			continue
		}
		conn.SetReadDeadline(start.Add(RESPONSE_TIMEOUT))
		if _, err := conn.Read(response); err != nil {
			result.timeouts++
			continue
		}
		result.latencies = append(result.latencies, float64(time.Since(start).Microseconds())/1000.0)
		result.successful++
	}

	return result
}

type benchmarkStats struct {
	allLatencies                                               []float64
	totalRequests, totalSuccessful, totalFailed, totalTimedout int64
	totalGets, totalUpdates                                    int64
	elapsed                                                    time.Duration
}

func runBenchmark(host string, port int, numThreads, requestsPerThread int) (benchmarkStats, error) {
	addr, err := net.ResolveUDPAddr("udp", fmt.Sprintf("%s:%d", host, port))
	if err != nil {
		return benchmarkStats{}, fmt.Errorf("resolve target: %w", err)
	}

	fmt.Printf("Preparing %d workers x %d requests/worker (%d total)...\n",
		numThreads, requestsPerThread, numThreads*requestsPerThread)

	// Generate every request for every thread BEFORE opening connections or
	// starting the clock. This is the fix for the request-generation-time
	// leaking into the measured window (see file header).
	perThreadRequests := make([][]request, numThreads)
	for t := 0; t < numThreads; t++ {
		reqs := make([]request, requestsPerThread)
		for i := range reqs {
			reqs[i] = buildRequest()
		}
		perThreadRequests[t] = reqs
	}

	conns := make([]*net.UDPConn, numThreads)
	for t := 0; t < numThreads; t++ {
		conn, err := net.DialUDP("udp", nil, addr)
		if err != nil {
			return benchmarkStats{}, fmt.Errorf("thread %d: dial: %w", t, err)
		}
		conns[t] = conn
	}
	defer func() {
		for _, c := range conns {
			c.Close()
		}
	}()

	fmt.Println("Starting benchmark...")
	resultsCh := make(chan workerResult, numThreads)

	start := time.Now()
	var wg sync.WaitGroup
	for t := 0; t < numThreads; t++ {
		wg.Add(1)
		go func(conn *net.UDPConn, reqs []request) {
			defer wg.Done()
			resultsCh <- runWorker(conn, reqs)
		}(conns[t], perThreadRequests[t])
	}
	wg.Wait()
	elapsed := time.Since(start)
	close(resultsCh)

	var stats benchmarkStats
	stats.elapsed = elapsed
	for r := range resultsCh {
		stats.allLatencies = append(stats.allLatencies, r.latencies...)
		stats.totalSuccessful += r.successful
		stats.totalFailed += r.failed
		stats.totalTimedout += r.timeouts
		stats.totalGets += r.gets
		stats.totalUpdates += r.updates
	}
	stats.totalRequests = stats.totalSuccessful + stats.totalFailed + stats.totalTimedout

	return stats, nil
}

func printResults(s benchmarkStats) {
	if len(s.allLatencies) == 0 {
		fmt.Println("No successful requests completed")
		return
	}

	sort.Float64s(s.allLatencies)

	var sum float64
	for _, lat := range s.allLatencies {
		sum += lat
	}
	avg := sum / float64(len(s.allLatencies))
	median := s.allLatencies[len(s.allLatencies)/2]
	p95 := s.allLatencies[int(float64(len(s.allLatencies))*0.95)]
	p99 := s.allLatencies[int(float64(len(s.allLatencies))*0.99)]

	fmt.Printf("\n=== BENCHMARK RESULTS ===\n")
	fmt.Printf("Total requests:       %d\n", s.totalRequests)
	fmt.Printf("Get requests:         %d\n", s.totalGets)
	fmt.Printf("Update requests:      %d\n", s.totalUpdates)
	fmt.Printf("Successful requests:  %d\n", s.totalSuccessful)
	fmt.Printf("Failed requests:      %d\n", s.totalFailed)
	fmt.Printf("Timedout requests:    %d\n", s.totalTimedout)
	fmt.Printf("Success rate:         %.4f%%\n", float64(s.totalSuccessful)/float64(s.totalRequests)*100)
	fmt.Printf("Total time:           %.2fs\n", s.elapsed.Seconds())
	fmt.Printf("Requests per second:  %.0f\n", float64(s.totalSuccessful)/s.elapsed.Seconds())
	fmt.Printf("Average latency:      %.3fms\n", avg)
	fmt.Printf("Median latency:       %.3fms\n", median)
	fmt.Printf("95th percentile:      %.3fms\n", p95)
	fmt.Printf("99th percentile:      %.3fms\n", p99)
	fmt.Printf("Min latency:          %.3fms\n", s.allLatencies[0])
	fmt.Printf("Max latency:          %.3fms\n", s.allLatencies[len(s.allLatencies)-1])
}

func main() {
	var (
		host     = flag.String("host", "127.0.0.1", "Server host")
		port     = flag.Int("port", 8080, "Server port")
		clients  = flag.Int("clients", 1, "Number of concurrent client connections")
		requests = flag.Int("requests", 1, "Requests per thread")
	)
	flag.Parse()

	stats, err := runBenchmark(*host, *port, *clients, *requests)
	if err != nil {
		fmt.Printf("benchmark failed: %v\n", err)
		return
	}
	printResults(stats)
}
