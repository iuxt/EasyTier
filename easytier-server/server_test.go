package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"github.com/pelletier/go-toml/v2"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"
)

const testAdmin = "test-admin-password-at-least-24"
const testToken = "test-registration-token-at-least-24"

func TestDeviceSettings(t *testing.T) {
	opts := defaultSettings()
	if err := opts.validate(); err != nil {
		t.Fatal(err)
	}
	for _, address := range []string{"https://vpn.example.com", "example.com:1234", "host/path", "0.0.0.0", "bad..host"} {
		opts.PublicAddress = address
		if opts.validate() == nil {
			t.Fatalf("accepted invalid address %q", address)
		}
	}
	opts = defaultSettings()
	opts.AutoIP = false
	opts.IPv4 = "invalid"
	if opts.validate() == nil {
		t.Fatal("invalid static IP accepted")
	}
	opts.IPv4 = "10.144.0.2/16"
	if err := opts.validate(); err != nil {
		t.Fatal(err)
	}
	opts.AllowInbound = false
	opts.EnableIPv6 = false
	opts.DirectConnection = false
	opts.UDPHolePunch = false
	encoded, err := buildConfig("", opts, "server-generated-network-secret", []string{"tcp://vpn.example.com:11010"})
	if err != nil {
		t.Fatal(err)
	}
	var doc map[string]any
	if err = toml.Unmarshal([]byte(encoded), &doc); err != nil {
		t.Fatal(err)
	}
	if doc["dhcp"] != false || doc["ipv4"] != "10.144.0.2/16" || len(doc["listeners"].([]any)) != 0 {
		t.Fatalf("settings not applied: %#v", doc)
	}
	flags := doc["flags"].(map[string]any)
	if flags["enable_ipv6"] != false || flags["disable_p2p"] != true || flags["disable_udp_hole_punching"] != true {
		t.Fatal(flags)
	}
	if identity := doc["network_identity"].(map[string]any); identity["network_secret"] != "server-generated-network-secret" {
		t.Fatal(identity)
	}
	if len(doc["peer"].([]any)) != 1 {
		t.Fatal("peer not generated")
	}
	encoded, err = buildConfig("", defaultSettings(), "server-generated-network-secret", nil)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(encoded, "0.0.0.0:11010") {
		t.Fatal("default listeners not generated")
	}
}
func TestExistingSettingsMigration(t *testing.T) {
	config := "hostname='old-device'\ndhcp=false\nipv4='10.144.0.5/16'\nlisteners=[]\n[flags]\nenable_ipv6=false\ndisable_p2p=true\ndisable_udp_hole_punching=true\n"
	opts, err := settingsFromConfig(config, "")
	if err != nil {
		t.Fatal(err)
	}
	if opts.Hostname != "old-device" || opts.AutoIP || opts.AllowInbound || opts.EnableIPv6 || opts.DirectConnection || opts.UDPHolePunch || opts.IPv4 != "10.144.0.5/16" {
		t.Fatalf("old device not migrated: %#v", opts)
	}
}
func TestHTTPWorkflowMySQL(t *testing.T) {
	dsn := os.Getenv("MYSQL_TEST_DSN")
	if dsn == "" {
		t.Skip("set MYSQL_TEST_DSN for real MySQL integration tests")
	}
	db, err := openDB(context.Background(), dsn)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	id := fmt.Sprintf("00000000-0000-4000-8000-%012x", time.Now().UnixNano()&0xffffffffffff)
	defer db.Exec("DELETE FROM devices WHERE id=?", id)
	defer db.Exec("DELETE FROM device_logs WHERE device_id=?", id)
	defer db.Exec("DELETE FROM device_preferences WHERE device_id=?", id)
	app := newServer(db, testAdmin, testToken, false)
	httpServer := httptest.NewServer(app.routes(http.NotFoundHandler()))
	defer httpServer.Close()
	cookie := ""
	call := func(method, path, bearer string, body any, expect int) []byte {
		t.Helper()
		var reader io.Reader
		if body != nil {
			b, _ := json.Marshal(body)
			reader = bytes.NewReader(b)
		}
		req, _ := http.NewRequest(method, httpServer.URL+path, reader)
		req.Header.Set("Content-Type", "application/json")
		if bearer != "" {
			req.Header.Set("Authorization", "Bearer "+bearer)
		}
		if cookie != "" {
			req.Header.Set("Cookie", cookie)
		}
		response, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		defer response.Body.Close()
		data, _ := io.ReadAll(response.Body)
		if response.StatusCode != expect {
			t.Fatalf("%s %s: got %d want %d: %s", method, path, response.StatusCode, expect, data)
		}
		if path == "/api/login" && expect == 200 {
			cookie = response.Header.Get("Set-Cookie")
		}
		return data
	}
	call("GET", "/api/admin/devices", "", nil, 401)
	call("POST", "/api/login", "", map[string]string{"password": "wrong"}, 401)
	call("POST", "/api/login", "", map[string]string{"password": testAdmin}, 200)
	secret := "test-unique-device-secret-at-least-32"
	registration := map[string]string{"id": id, "secret": secret, "hostname": "测试设备", "platform": "linux", "version": "test"}
	call("POST", "/api/client/register", "wrong", registration, 401)
	call("POST", "/api/client/register", testToken, registration, 200)
	call("POST", "/api/client/register", testToken, registration, 200)
	registration["secret"] = "attacker-secret-at-least-thirty-two-characters"
	call("POST", "/api/client/register", testToken, registration, 403)
	syncPath := "/api/client/" + id + "/sync"
	runtime := map[string]any{"applied_revision": 0, "runtime_status": "waiting", "runtime_error": ""}
	call("POST", syncPath, "wrong", runtime, 401)
	sync := call("POST", syncPath, secret, runtime, 200)
	if !strings.Contains(string(sync), `"config":""`) || !strings.Contains(string(sync), `"pending"`) {
		t.Fatalf("pending device leaked config: %s", sync)
	}
	updatePath := "/api/admin/devices/" + id
	call("PUT", updatePath, "", map[string]any{"status": "approved", "config": "", "revision": 1}, 400)
	opts := defaultSettings()
	call("PUT", updatePath, "", map[string]any{"status": "approved", "revision": 1}, 400)
	call("PUT", updatePath, "", map[string]any{"status": "approved", "settings": opts, "revision": 1}, 200)
	call("PUT", updatePath, "", map[string]any{"status": "revoked", "settings": opts, "revision": 1}, 409)
	sync = call("POST", syncPath, secret, runtime, 200)
	if !strings.Contains(string(sync), `"approved"`) || !strings.Contains(string(sync), "network_secret") {
		t.Fatalf("approval missing config: %s", sync)
	}
	var networkSecret string
	if err = db.QueryRow("SELECT secret FROM managed_network WHERE id=1").Scan(&networkSecret); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(sync), networkSecret) {
		t.Fatal("server network identity not applied")
	}
	adminList := call("GET", "/api/admin/devices", "", nil, 200)
	if strings.Contains(string(adminList), "network_secret") || strings.Contains(string(adminList), "listeners") || strings.Contains(string(adminList), networkSecret) || strings.Contains(string(adminList), `"config":`) {
		t.Fatal("Web API exposes internal configuration")
	}
	if !strings.Contains(string(adminList), `"auto_ip":true`) {
		t.Fatal("Web settings missing")
	}
	runtime["applied_revision"] = 2
	runtime["runtime_status"] = "running"
	call("POST", syncPath, secret, runtime, 200)
	entry := map[string]string{"id": "00000000-0000-4000-8000-000000000001", "timestamp": time.Now().UTC().Format(time.RFC3339), "level": "INFO", "target": "test", "message": "central log 中文"}
	logPath := "/api/client/" + id + "/logs"
	body := map[string]any{"logs": []any{entry}}
	call("POST", logPath, secret, body, 200)
	call("POST", logPath, secret, body, 200)
	logs := call("GET", updatePath+"/logs", "", nil, 200)
	var entries []logEntry
	if err = json.Unmarshal(logs, &entries); err != nil || len(entries) != 1 {
		t.Fatalf("log idempotency failed: %s", logs)
	}
	call("PUT", updatePath, "", map[string]any{"status": "revoked", "revision": 2}, 200)
	sync = call("POST", syncPath, secret, runtime, 200)
	if !strings.Contains(string(sync), `"config":""`) || !strings.Contains(string(sync), `"revoked"`) {
		t.Fatalf("revoked device leaked config: %s", sync)
	}
	// Re-registering must preserve the administrator's decision.
	registration["secret"] = secret
	call("POST", "/api/client/register", testToken, registration, 200)
	sync = call("POST", syncPath, secret, runtime, 200)
	if !strings.Contains(string(sync), `"revoked"`) {
		t.Fatalf("registration reset approval: %s", sync)
	}
	req, _ := http.NewRequest("PUT", httpServer.URL+updatePath, strings.NewReader(`{}`))
	req.Header.Set("Origin", "https://attacker.invalid")
	req.Header.Set("Cookie", cookie)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if resp.StatusCode != 403 {
		t.Fatal("cross-origin mutation allowed")
	}
	if err = initializeNetwork(context.Background(), db); err != nil {
		t.Fatal(err)
	}
	var nextSecret string
	db.QueryRow("SELECT secret FROM managed_network WHERE id=1").Scan(&nextSecret)
	if nextSecret != networkSecret {
		t.Fatal("network secret changed after restart")
	}
	call("POST", "/api/logout", "", map[string]any{}, 200)
	call("GET", "/api/admin/devices", "", nil, 401)
}
func TestLoginRateLimitAndJSON(t *testing.T) {
	app := newServer(nil, testAdmin, testToken, false)
	handler := app.routes(http.NotFoundHandler())
	req := httptest.NewRequest("POST", "/api/login", strings.NewReader(`{"password":"x"}`))
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, req)
	if rec.Code != 415 {
		t.Fatal(rec.Code)
	}
	for i := 0; i < 11; i++ {
		req = httptest.NewRequest("POST", "/api/login", strings.NewReader(`{"password":"x"}`))
		req.Header.Set("Content-Type", "application/json")
		rec = httptest.NewRecorder()
		handler.ServeHTTP(rec, req)
	}
	if rec.Code != 429 {
		t.Fatal("login not rate limited")
	}
}
