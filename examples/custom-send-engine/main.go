// A local-only example of the version 1 custom send-engine protocol.
// It returns a synthetic response and never connects to the request URL.
package main

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
)

type header struct {
	Name  string `json:"name"`
	Value string `json:"value"`
}

type request struct {
	Method     string   `json:"method"`
	URL        string   `json:"url"`
	Headers    []header `json:"headers"`
	BodyBase64 string   `json:"bodyBase64"`
}

type input struct {
	Request request `json:"request"`
	Preset  string  `json:"preset"`
}

func main() {
	var data input
	if err := json.NewDecoder(os.Stdin).Decode(&data); err != nil {
		_ = json.NewEncoder(os.Stdout).Encode(map[string]any{"error": fmt.Sprintf("invalid request: %v", err)})
		return
	}
	body := fmt.Sprintf("custom engine received %s %s using %s; bodyBase64=%s", data.Request.Method, data.Request.URL, data.Preset, data.Request.BodyBase64)
	_ = json.NewEncoder(os.Stdout).Encode(map[string]any{
		"response": map[string]any{
			"status": 200, "version": "HTTP/1.1",
			"headers": []header{{Name: "Content-Type", Value: "text/plain; charset=utf-8"}},
			"bodyBase64": base64.StdEncoding.EncodeToString([]byte(body)),
			"rawHeadBase64": nil,
		},
		"notes": []string{"Example engine: response generated locally; no network request was sent."},
		"error": nil,
	})
}
