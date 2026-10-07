// Independent client for the privileged, opt-in TUN smoke test.
package main

import (
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"net/http"
	"os"
	"time"

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
