package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"strconv"
	"strings"

	http "github.com/sardanioss/http"
	"github.com/sardanioss/httpcloak/fingerprint"
	tr "github.com/sardanioss/httpcloak/transport"
)

// No application HTTP bytes have been sent when HTTP2Transport produces its
// typed ALPNMismatchError. Other errors, including response/read failures, are
// not negotiation signals and must never cause another POST to be issued.
func selectRequestProtocol(in Input, imported tr.Protocol) (tr.Protocol, bool, error) {
	requested := in.Protocol
	if requested == "" {
		requested = "auto"
	}
	if requested != "auto" && requested != "h1" && requested != "h2" {
		return 0, false, fmt.Errorf("protocol must be auto, h1 or h2")
	}
	if strings.HasPrefix(in.Request.URL, "http://") {
		if requested == "h2" {
			return 0, false, fmt.Errorf("explicit h2 requires an HTTPS URL")
		}
		return tr.ProtocolHTTP1, false, nil
	}
	if in.Request.TLS.ClientHelloHex != nil {
		// Retain the existing import contract: a hello offering h2 is strict H2,
		// even if it also offers http/1.1. Do not rewrite an imported ALPN list.
		if (requested == "h1" && imported != tr.ProtocolHTTP1) || (requested == "h2" && imported != tr.ProtocolHTTP2) {
			return 0, false, fmt.Errorf("explicit protocol conflicts with imported ClientHello")
		}
		return imported, false, nil
	}
	if requested == "h1" {
		return tr.ProtocolHTTP1, false, nil
	}
	return tr.ProtocolHTTP2, requested == "auto", nil
}

type negotiatedResponse struct {
	io.ReadCloser
	StatusCode int
	Headers    map[string][]string
	Protocol   string
	// DoStream/Do decode Content-Encoding. A handed-off H1 connection returns
	// exact encoded entity bytes instead; retain its encoding/length headers.
	decodedBody bool
}

func sendNegotiated(ctx context.Context, transport *tr.Transport, request *tr.Request, protocol tr.Protocol, allowHTTP1 bool, fallbackHeaders []fingerprint.HeaderPair) (*negotiatedResponse, error) {
	if protocol != tr.ProtocolHTTP1 && protocol != tr.ProtocolHTTP2 {
		return nil, fmt.Errorf("TCP negotiation requires an explicit initial h1 or h2 transport")
	}
	transport.SetProtocol(protocol)
	response, err := transport.DoStream(ctx, request)
	if err == nil {
		return &negotiatedResponse{response, response.StatusCode, response.Headers, response.Protocol, true}, nil
	}
	return finishALPN(ctx, transport, request, allowHTTP1, fallbackHeaders, err)
}

func finishALPN(ctx context.Context, transport *tr.Transport, request *tr.Request, allowHTTP1 bool, fallbackHeaders []fingerprint.HeaderPair, err error) (*negotiatedResponse, error) {
	var mismatch *tr.ALPNMismatchError
	if !errors.As(err, &mismatch) || mismatch.TLSConn == nil {
		return nil, err
	}
	// Forced H2/imported hellos still fail, but do not leak the returned socket.
	// This also owns the handoff until RoundTripWithTLSConn transfers it to the
	// response body's close wrapper. No redial or second handshake is needed.
	owned := true
	defer func() {
		if owned {
			mismatch.TLSConn.Close()
		}
	}()
	if !allowHTTP1 || mismatch.Expected != "h2" || (mismatch.Negotiated != "http/1.1" && mismatch.Negotiated != "") {
		return nil, err
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	bodyLength := int64(len(request.Body))
	var body io.Reader = bytes.NewReader(request.Body)
	if request.BodyReader != nil {
		if request.GetBody == nil {
			return nil, err
		}
		bodyLength = request.BodyLength
		if bodyLength < 0 {
			return nil, fmt.Errorf("invalid streaming body length")
		}
		body, err = request.GetBody()
		if err != nil {
			return nil, err
		}
	}
	// H2 may contain an internal empty User-Agent solely to suppress the Go
	// encoder's default. Hand the H1 writer the unmodified editor fields.
	pairs, prepareErr := prepareH1Headers(fallbackHeaders, bodyLength)
	if prepareErr != nil {
		return nil, prepareErr
	}
	method := request.Method
	if method == "" {
		method = "GET"
	}
	httpRequest, err := http.NewRequestWithContext(ctx, method, request.URL, body)
	if err != nil {
		return nil, err
	}
	if request.BodyReader != nil {
		httpRequest.ContentLength = bodyLength
	}
	if len(pairs) > 0 {
		order := make([]string, 0, len(pairs))
		for _, pair := range pairs {
			httpRequest.Header[pair.Key] = append(httpRequest.Header[pair.Key], pair.Value)
			order = append(order, pair.Key)
		}
		httpRequest.Header[http.HeaderOrderKey] = order
		// Pinned httpcloak v1.7.2's H1 exact-header marker. It is consumed by
		// the writer, never emitted, and suppresses synthesized Connection.
		httpRequest.Header["Exact-Headers:"] = []string{"1"}
	}
	raw, err := transport.GetHTTP1Transport().RoundTripWithTLSConn(httpRequest, mismatch.TLSConn, mismatch.Host, mismatch.Port)
	if err != nil {
		return nil, err
	}
	headers := make(map[string][]string, len(raw.Header))
	for name, values := range raw.Header {
		if !strings.HasSuffix(name, ":") {
			headers[name] = append([]string(nil), values...)
		}
	}
	// The body owns the live connection now. Do not close it at function exit;
	// the response Close below (including errors/cancellation) releases it.
	owned = false
	return &negotiatedResponse{raw.Body, raw.StatusCode, headers, "h1", false}, nil
}

func prepareH1Headers(input []fingerprint.HeaderPair, bodyLength int64) ([]fingerprint.HeaderPair, error) {
	joined, _ := joinCookieFields(input)
	pairs := append([]fingerprint.HeaderPair(nil), joined...)
	lengthSeen := false
	for index := range pairs {
		pair := &pairs[index]
		if strings.EqualFold(pair.Key, "transfer-encoding") {
			return nil, fmt.Errorf("H1 exact body does not accept Transfer-Encoding")
		}
		if !strings.EqualFold(pair.Key, "content-length") {
			continue
		}
		if lengthSeen {
			return nil, fmt.Errorf("H1 body requires a single Content-Length")
		}
		lengthSeen = true
		if pair.Value == "" || strings.IndexFunc(pair.Value, func(c rune) bool { return c < '0' || c > '9' }) >= 0 {
			return nil, fmt.Errorf("Invalid H1 Content-Length")
		}
		length, err := strconv.ParseUint(pair.Value, 10, 64)
		if err != nil || length != uint64(bodyLength) {
			return nil, fmt.Errorf("H1 Content-Length does not match body bytes")
		}
		// The pinned H1 writer's framing branch looks up this exact map key.
		// Leaving a lowercase key emits both a computed CL and the original in
		// its remainder pass. Only framing spelling changes, at the same slot.
		pair.Key = "Content-Length"
	}
	if !lengthSeen && bodyLength > 0 {
		pairs = append(pairs, fingerprint.HeaderPair{Key: "Content-Length", Value: strconv.FormatInt(bodyLength, 10)})
	}
	return pairs, nil
}
