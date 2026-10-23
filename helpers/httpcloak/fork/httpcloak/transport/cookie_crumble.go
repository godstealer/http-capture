package transport

import "strings"

// crumbleCookie splits a single Cookie header value into per-pair "crumbs",
// reproducing Chromium's cookie-crumbling exactly. Chrome (and Firefox) emit
// one "cookie" header field per cookie-pair on the wire for better HPACK/QPACK
// compression (RFC 9113 8.2.3). On H2 the sardanioss/net HPACK encoder does
// this itself when DisableCookieSplit is false; on H3 quic-go's QPACK writer
// does not, so the H3 transport pre-splits with this helper and hands quic-go
// one value per crumb.
//
// The algorithm matches Chromium HpackEncoder::CookieToCrumbs /
// QPACK ValueSplittingHeaderList byte-for-byte:
//  1. trim leading/trailing ' ' and '\t' from the whole value
//  2. split on each ';'
//  3. after each ';', drop EXACTLY ONE following space (an if, not a while)
//
// Examples:
//
//	"a=1; b=2; c=3" -> ["a=1", "b=2", "c=3"]
//	"a=1;b=2"       -> ["a=1", "b=2"]
//	"a=1;  b=2"     -> ["a=1", " b=2"]   // only one space consumed
//	"a=1"           -> ["a=1"]
//
// An empty value (after trimming) yields nil; httpcloak never sends an empty
// Cookie header, and emitting an empty "cookie" field would be a divergence.
func crumbleCookie(value string) []string {
	value = strings.Trim(value, " \t")
	if value == "" {
		return nil
	}

	var crumbs []string
	pos := 0
	for {
		rel := strings.IndexByte(value[pos:], ';')
		if rel < 0 {
			crumbs = append(crumbs, value[pos:])
			break
		}
		end := pos + rel
		crumbs = append(crumbs, value[pos:end])
		pos = end + 1
		// Chrome consumes exactly one optional space after the ';'.
		if pos < len(value) && value[pos] == ' ' {
			pos++
		}
	}
	return crumbs
}
