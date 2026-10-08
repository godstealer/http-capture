// Independent client for the privileged, opt-in TUN smoke test.
package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"time"

	"github.com/sagernet/quic-go"
	"github.com/sagernet/quic-go/http3"
)

func run() error {
	if len(os.Args) != 3 || os.Args[1] != "--client" {
		return fmt.Errorf("usage: h3-test-client --client CA.pem")
	}
	pem, err := os.ReadFile(os.Args[2])
	if err != nil {
		return err
	}
	roots := x509.NewCertPool()
	if !roots.AppendCertsFromPEM(pem) {
		return fmt.Errorf("invalid test CA")
	}
	transport := &http3.Transport{TLSClientConfig: &tls.Config{RootCAs: roots}}
	// The isolated smoke test routes a reserved test address through the real TUN.
	// TLS still verifies the URL hostname against the supplied test CA.
	if address := os.Getenv("HTTP_CAPTURE_TUN_TEST_ADDRESS"); address != "" {
		if os.Getenv("HTTP_CAPTURE_TUN_TEST_UDP_PROBES") == "1" {
			for _, destination := range []string{"198.18.0.10:53", "198.18.0.10:80", "198.18.0.10:123", "198.18.0.10:443", "198.18.0.10:853", "198.18.0.10:8443", "198.18.0.10:44444"} {
				probe, err := net.Dial("udp4", destination)
				if err != nil {
					return err
				}
				// Keep the socket alive so process attribution can find its owner.
				defer probe.Close()
				for attempt := 0; attempt < 3; attempt++ {
					_, err = probe.Write([]byte("tun-diagnostic"))
					if err != nil {
						break
					}
					time.Sleep(50 * time.Millisecond)
				}
				fmt.Fprintf(os.Stderr, "udp-probe destination=%s local=%s result=%v\n", destination, probe.LocalAddr(), err)
			}
		}
		transport.Dial = func(ctx context.Context, _ string, cfg *tls.Config, qc *quic.Config) (*quic.Conn, error) {
			return quic.DialAddrEarly(ctx, address, cfg, qc)
		}
	}
	defer transport.Close()
	client := &http.Client{Transport: transport, Timeout: 25 * time.Second}
	response, err := client.Get("https://tls3.peet.ws/api/all")
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if _, err = io.Copy(io.Discard, response.Body); err != nil {
		return err
	}
	if response.StatusCode != 200 || response.ProtoMajor != 3 {
		return fmt.Errorf("unexpected response: %s %s", response.Proto, response.Status)
	}
	fmt.Println("client=200/HTTP3")
	return nil
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
