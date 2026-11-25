package main

type engineInput struct {
	Request  engineRequest   `json:"request"`
	Preset   string          `json:"preset"`
	Proxy    *engineProxy    `json:"proxy"`
	Security *engineSecurity `json:"security"`
}

type engineRequest struct {
	Method        string         `json:"method"`
	URL           string         `json:"url"`
	Headers       []engineHeader `json:"headers"`
	PseudoHeaders []engineHeader `json:"pseudoHeaders"`
	BodyBase64    string         `json:"bodyBase64"`
	TLS           engineTLS      `json:"tls"`
}

type engineHeader struct {
	Name     string `json:"name"`
	Value    string `json:"value"`
	Disabled bool   `json:"disabled,omitempty"`
}

type engineTLS struct {
	Preset            string  `json:"preset"`
	Version           string  `json:"version"`
	ClientHelloHex    *string `json:"clientHelloHex"`
	BrowserVersion    *string `json:"browserVersion"`
	CipherList        *string `json:"cipherList"`
	SigalgsList       *string `json:"sigalgsList"`
	CurvesList        *string `json:"curvesList"`
	Grease            *bool   `json:"grease"`
	PermuteExtensions *bool   `json:"permuteExtensions"`
}

type engineProxy struct {
	URL      string `json:"url"`
	Username string `json:"username"`
	Password string `json:"password"`
}

type engineSecurity struct {
	TrustedCAPEM         []string `json:"trustedCaPem"`
	ClientCertificatePEM string   `json:"clientCertificatePem"`
	ClientPrivateKeyPEM  string   `json:"clientPrivateKeyPem"`
}

type engineReply struct {
	Response *engineResponse `json:"response"`
	Error    string          `json:"error,omitempty"`
}

type engineResponse struct {
	Status             int               `json:"status"`
	Version            string            `json:"version"`
	Headers            []engineHeader    `json:"headers"`
	Trailers           []engineHeader    `json:"trailers"`
	BodyBase64         string            `json:"bodyBase64"`
	SentRequestHeaders []engineHeader    `json:"sentRequestHeaders,omitempty"`
	UpstreamTLS        *engineTLSDetails `json:"upstreamTls,omitempty"`
	TLSVersion         string            `json:"tlsVersion,omitempty"`
}

type engineTLSDetails struct {
	Version             string              `json:"version"`
	CipherSuite         string              `json:"cipherSuite"`
	ALPN                string              `json:"alpn,omitempty"`
	ServerName          string              `json:"serverName,omitempty"`
	OfferedCipherSuites []string            `json:"offeredCipherSuites"`
	OfferedALPN         []string            `json:"offeredAlpn"`
	SignatureSchemes    []string            `json:"signatureSchemes"`
	SupportedGroups     []string            `json:"supportedGroups"`
	Certificates        []engineCertificate `json:"certificates"`
}

type engineCertificate struct {
	Subject   string   `json:"subject"`
	Issuer    string   `json:"issuer"`
	Serial    string   `json:"serial"`
	NotBefore string   `json:"notBefore"`
	NotAfter  string   `json:"notAfter"`
	SHA256    string   `json:"sha256"`
	DNSNames  []string `json:"dnsNames"`
	DERBase64 string   `json:"derBase64"`
}
