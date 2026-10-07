// This helper links sing-box and is distributed under GPL-3.0-or-later.
package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/adapter/certificate"
	"github.com/sagernet/sing-box/adapter/endpoint"
	"github.com/sagernet/sing-box/adapter/inbound"
	"github.com/sagernet/sing-box/adapter/outbound"
	"github.com/sagernet/sing-box/adapter/service"
	"github.com/sagernet/sing-box/dns"
	"github.com/sagernet/sing-box/dns/transport/local"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing-box/protocol/direct"
	httpout "github.com/sagernet/sing-box/protocol/http"
	"github.com/sagernet/sing-box/protocol/tun"
	sjson "github.com/sagernet/sing/common/json"
)

func run() error {
	if len(os.Args) > 1 && os.Args[1] == "--version" {
		fmt.Println("http-capture-tun/1 sing-box/1.14.0")
		return nil
	}
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 4096), 256*1024)
	if !scanner.Scan() {
		return fmt.Errorf("missing configuration")
	}
	config := append([]byte(nil), scanner.Bytes()...)
	ir := inbound.NewRegistry()
	tun.RegisterInbound(ir)
	or := outbound.NewRegistry()
	direct.RegisterOutbound(or)
	httpout.RegisterOutbound(or)
	dr := dns.NewTransportRegistry()
	local.RegisterTransport(dr)
	ctx := box.Context(context.Background(), ir, or, endpoint.NewRegistry(), dr, service.NewRegistry(), certificate.NewRegistry())
	options, err := sjson.UnmarshalExtendedContext[option.Options](ctx, config)
	if err != nil {
		return err
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	instance, err := box.New(box.Options{Context: ctx, Options: options})
	if err != nil {
		return err
	}
	defer instance.Close()
	if len(os.Args) > 1 && os.Args[1] == "--check" {
		return json.NewEncoder(os.Stdout).Encode(map[string]bool{"valid": true})
	}
	// stdin is a lifetime lease. Closing the app or its pipe stops TUN and restores its routes.
	lease := make(chan struct{})
	go func() { scanner.Scan(); close(lease) }()
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, os.Interrupt, syscall.SIGTERM)
	defer signal.Stop(signals)
	go func() {
		select {
		case <-lease:
			cancel()
		case <-ctx.Done():
		}
	}()
	if err = instance.Start(); err != nil {
		return err
	}
	if err = json.NewEncoder(os.Stdout).Encode(map[string]bool{"ready": true}); err != nil {
		return err
	}
	select {
	case <-lease:
	case <-signals:
	case <-ctx.Done():
	}
	cancel()
	return nil
}
func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
