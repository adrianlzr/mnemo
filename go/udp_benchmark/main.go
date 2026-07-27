package main

import (
	"crypto/rand"
	"encoding/binary"
	"flag"
	"fmt"
	"math/big"
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

var OP_RAND_RANGE = *big.NewInt(2)

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

func NewOPInt(op int) OP {
	switch op {
	case 1:
		return OP_GET
	case 2:
		return OP_UPDATE
	default:
		return OP_UNKNOWN
	}
}

const (
	OP_SIZE             = 1
	KEY_SIZE            = 8
	VAL_SIZE            = 36
	GET_REQUEST_SIZE    = OP_SIZE + KEY_SIZE
	UPDATE_REQUEST_SIZE = GET_REQUEST_SIZE + VAL_SIZE
	RESPONSE_SIZE       = VAL_SIZE
	RESPONSE_TIMEOUT_MS = time.Millisecond * 10
)

type Benchmark struct {
	serverAddr string
}

type BenchmarkResult struct {
	latencies  []float64
	successful int64
	failures   int64
	timeouts   int64
	total      int64
	gets       int64
	updates    int64
}

func NewBenchmark(serverHost string, serverPort int) Benchmark {
	serverAddr := fmt.Sprintf("%s:%d", serverHost, serverPort)

	return Benchmark{
		serverAddr: serverAddr,
	}
}

type BenchmarkWorker struct {
	conn    *net.UDPConn
	results chan<- BenchmarkResult
}

func request_size(op OP) int {
	if op == OP_GET {
		return GET_REQUEST_SIZE
	}
	return UPDATE_REQUEST_SIZE
}

func NewBenchmarkWorker(
	b *Benchmark,
	results chan<- BenchmarkResult,
) (*BenchmarkWorker, error) {
	// Create UDP connection
	addr, err := net.ResolveUDPAddr("udp", b.serverAddr)
	if err != nil {
		return nil, err
	}

	conn, err := net.DialUDP("udp", nil, addr)
	if err != nil {
		return nil, err
	}

	// Set socket options for high performance
	// if err := conn.SetWriteBuffer(UPDATE_REQUEST_SIZE); err != nil {
	// 	log.Printf("Warning: failed to set write buffer: %v", err)
	// }
	// if err := conn.SetReadBuffer(RESPONSE_SIZE); err != nil {
	// 	log.Printf("Warning: failed to set read buffer: %v", err)
	// }

	return &BenchmarkWorker{
		conn:    conn,
		results: results,
	}, nil
}

func (bw *BenchmarkWorker) bench(numRequests int) {
	latencies := make([]float64, 0, numRequests)
	successful := int64(0)
	failures := int64(0)
	timeouts := int64(0)
	gets := int64(0)
	updates := int64(0)

	// Pre-allocate buffers to avoid allocations in the loop
	requests := make([][]byte, numRequests)
	key := make([]byte, KEY_SIZE)
	val := make([]byte, VAL_SIZE)
	response := make([]byte, RESPONSE_SIZE)

	// generate random requests
	for i := range numRequests {
		request := make([]byte, UPDATE_REQUEST_SIZE) // move inside the loop

		// Operation
		op := randOP()
		request[0] = byte(op)

		// Key
		randKey(key)
		copy(request[OP_SIZE:OP_SIZE+KEY_SIZE], key)

		if op == OP_UPDATE {
			// Value
			randVal(val)
			copy(request[OP_SIZE+KEY_SIZE:OP_SIZE+KEY_SIZE+VAL_SIZE], val)
			updates++
		} else {
			gets++
		}

		requests[i] = request
	}

	// Execute the requests and measure
	var wg sync.WaitGroup
	sem := make(chan struct{}, 10)
	for i := range numRequests {
		sem <- struct{}{}
		wg.Add(1)
		go func() {
			defer wg.Done()
			defer func() {
				<-sem
			}()
			start := time.Now()
			req := requests[i]
			_, err := bw.conn.Write(req)
			if err != nil {
				failures++
				return
			}
			bw.conn.SetReadDeadline(start.Add(RESPONSE_TIMEOUT_MS))
			_, err = bw.conn.Read(response)
			latency := float64(time.Since(start).Nanoseconds()) / 1e6
			latencies = append(latencies, latency)
			if err == nil {
				successful++
			} else {
				timeouts++
			}
		}()
	}
	wg.Wait()

	bw.results <- BenchmarkResult{
		latencies:  latencies,
		successful: successful,
		failures:   failures,
		timeouts:   timeouts,
		total:      int64(numRequests),
		gets:       gets,
		updates:    updates,
	}
}

func randOP() OP {
	n, err := rand.Int(rand.Reader, &OP_RAND_RANGE)
	if err != nil {
		return OP_GET
	}
	return NewOPInt(int(n.Int64()) + 1)

}

func randKey(key []byte) {
	bundleID := uuid.New().String()
	idfa := uuid.New().String()

	concat := bundleID + idfa

	hash := xxh3.Hash([]byte(concat)) // uint64

	binary.BigEndian.PutUint64(key, hash)
}

func randVal(val []byte) {
	copy(val, []byte(uuid.New().String()))
}

func (bw *BenchmarkWorker) close() {
	bw.conn.Close()
}

func (b *Benchmark) runBenchmark(numWorkers, requestsPerThread int, duration time.Duration) error {

	fmt.Printf("Starting benchmark: duration: %v, %d workers, %d requests/worker\n", duration, numWorkers, requestsPerThread)

	startTime := time.Now()
	results := make(chan BenchmarkResult, numWorkers)

	// Progress reporting
	progressTicker := time.NewTicker(5 * time.Second)
	defer progressTicker.Stop()

	go func() {
		for range progressTicker.C {
			elapsed := time.Since(startTime)
			fmt.Printf("Progress: %v elapsed\n", elapsed.Round(time.Second))
		}
	}()

	// Start worker goroutines with their own connections
	var wg sync.WaitGroup
	for range numWorkers {
		wg.Add(1)
		go func() {
			defer wg.Done()
			// Create a new connection for each worker
			workerBenchmark, err := NewBenchmarkWorker(b, results)
			if err != nil {
				fmt.Printf("Failed to create worker connection: %v\n", err)
				return
			}
			defer workerBenchmark.close()

			workerBenchmark.bench(requestsPerThread)
		}()
	}

	// Wait for completion or timeout
	done := make(chan struct{})
	go func() {
		wg.Wait()
		close(done)
	}()

	select {
	case <-done:
		// All workers completed
		fmt.Println("All workers completed")
	case <-time.After(duration):
		// Timeout reached
		fmt.Println("Benchmark timed out - collecting partial results")
	}

	close(results)

	// Collect results
	var allLatencies []float64
	var totalSuccessful int64
	var totalFailed int64
	var totalTimedout int64
	var totalRequests int64
	var totalGetRequests int64
	var totalUpdateRequests int64

	for result := range results {
		allLatencies = append(allLatencies, result.latencies...)
		totalSuccessful += result.successful
		totalFailed += result.failures
		totalTimedout += result.timeouts
		totalRequests += result.total
		totalGetRequests += result.gets
		totalUpdateRequests += result.updates
	}

	elapsed := time.Since(startTime)

	if len(allLatencies) > 0 {
		// Sort latencies for percentile calculations
		sort.Float64s(allLatencies)

		// Calculate statistics
		var sum float64
		for _, lat := range allLatencies {
			sum += lat
		}
		avgLatency := sum / float64(len(allLatencies))

		medianLatency := allLatencies[len(allLatencies)/2]
		p95Latency := allLatencies[int(float64(len(allLatencies))*0.95)]
		p99Latency := allLatencies[int(float64(len(allLatencies))*0.99)]

		fmt.Printf("\n=== BENCHMARK RESULTS ===\n")
		fmt.Printf("Total requests:       %d\n", totalRequests)
		fmt.Printf("Get requests:         %d\n", totalGetRequests)
		fmt.Printf("Update requests:      %d\n", totalUpdateRequests)
		fmt.Printf("Successful requests:  %d\n", totalSuccessful)
		fmt.Printf("Failed requests:      %d\n", totalFailed)
		fmt.Printf("Timedout requests:    %d\n", totalTimedout)
		fmt.Printf("Success rate:         %.4f%%\n", float64(totalSuccessful)/float64(totalRequests)*100)
		fmt.Printf("Total time:           %.2fs\n", elapsed.Seconds())
		fmt.Printf("Requests per second:  %.0f\n", float64(totalSuccessful)/elapsed.Seconds())
		fmt.Printf("Average latency:      %.3fms\n", avgLatency)
		fmt.Printf("Median latency:       %.3fms\n", medianLatency)
		fmt.Printf("95th percentile:      %.3fms\n", p95Latency)
		fmt.Printf("99th percentile:      %.3fms\n", p99Latency)
		fmt.Printf("Min latency:          %.3fms\n", allLatencies[0])
		fmt.Printf("Max latency:          %.3fms\n", allLatencies[len(allLatencies)-1])
	} else {
		fmt.Println("No successful requests completed")
	}
	return nil
}

func main() {
	var (
		host     = flag.String("host", "127.0.0.1", "Server host")
		port     = flag.Int("port", 8080, "Server port")
		clients  = flag.Int("clients", 1, "Number of client threads")
		requests = flag.Int("requests", 1, "Requests per thread")
		duration = flag.Duration("duration", 3600*time.Second, "Test duration")
	)
	flag.Parse()

	// Random number generator is automatically seeded in Go 1.20+

	benchmark := NewBenchmark(*host, *port)

	benchmark.runBenchmark(*clients, *requests, *duration)
}
