package main

import (
	"github.com/sardanioss/httpcloak/fingerprint"
	"testing"
)

func TestCapabilitiesExposeRealChrome155AndRetainOlderVersions(t *testing.T) {
	caps := capabilities()
	versions := caps["browserVersions"].(map[string][]int)
	if len(versions["chrome"]) == 0 || versions["chrome"][0] != 155 {
		t.Fatalf("expected Chrome 155 as newest supported version: %v", versions)
	}
	for _, version := range []int{150, 152, 153, 154, 155} {
		found := false
		for _, value := range versions["chrome"] {
			if value == version {
				found = true
			}
		}
		if !found {
			t.Fatalf("missing Chrome %d", version)
		}
	}
	preset := fingerprint.GetStrict("chrome-155-windows")
	if preset == nil {
		t.Fatal("capability must correspond to a real compiled profile")
	}
	// Importing a captured request must keep its exact UA/Accept values; the
	// new preset only supplies TLS/H2 behavior and never injects browser headers.
	if !caps["certificateRoots"].(bool) || !caps["customClientHello"].(bool) {
		t.Fatal("upgrading must retain adapter capabilities")
	}
}
