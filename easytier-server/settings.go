package main

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"net"
	"net/netip"
	"net/url"
	"strconv"
	"strings"

	"github.com/pelletier/go-toml/v2"
)

// DeviceSettings is the complete administrator-facing settings contract.
// Network credentials and listener URLs never cross the Web API boundary.
type DeviceSettings struct {
	Hostname         string `json:"hostname"`
	AutoIP           bool   `json:"auto_ip"`
	IPv4             string `json:"ipv4"`
	AllowInbound     bool   `json:"allow_inbound"`
	EnableIPv6       bool   `json:"enable_ipv6"`
	DirectConnection bool   `json:"direct_connection"`
	UDPHolePunch     bool   `json:"udp_hole_punch"`
	PeerID           string `json:"peer_id"`
	PublicAddress    string `json:"public_address"`
}

func defaultSettings() DeviceSettings {
	return DeviceSettings{AutoIP: true, AllowInbound: true, EnableIPv6: true, DirectConnection: true, UDPHolePunch: true}
}
func settingsFromConfig(config, preferences string) (DeviceSettings, error) {
	opts := defaultSettings()
	var doc map[string]any
	if config != "" {
		if err := toml.Unmarshal([]byte(config), &doc); err != nil {
			return opts, errors.New("设备原有设置无法读取")
		}
	}
	if v, ok := doc["hostname"].(string); ok {
		opts.Hostname = v
	}
	if v, ok := doc["dhcp"].(bool); ok {
		opts.AutoIP = v
	}
	if v, ok := doc["ipv4"].(string); ok {
		opts.IPv4 = v
	}
	if v, ok := doc["listeners"].([]any); ok {
		opts.AllowInbound = len(v) > 0
	}
	if flags, ok := doc["flags"].(map[string]any); ok {
		if v, ok := flags["enable_ipv6"].(bool); ok {
			opts.EnableIPv6 = v
		}
		if v, ok := flags["disable_p2p"].(bool); ok {
			opts.DirectConnection = !v
		}
		if v, ok := flags["disable_udp_hole_punching"].(bool); ok {
			opts.UDPHolePunch = !v
		}
	}
	if preferences != "" {
		if err := json.Unmarshal([]byte(preferences), &opts); err != nil {
			return opts, errors.New("设备设置无法读取")
		}
	}
	return opts, nil
}
func (opts *DeviceSettings) validate() error {
	opts.Hostname = strings.TrimSpace(opts.Hostname)
	opts.IPv4 = strings.TrimSpace(opts.IPv4)
	opts.PublicAddress = strings.TrimSpace(opts.PublicAddress)
	if len(opts.Hostname) > 32 {
		return errors.New("设备名称最多 32 字节")
	}
	if !opts.AutoIP {
		prefix, err := netip.ParsePrefix(opts.IPv4)
		if err != nil || !prefix.Addr().Is4() || prefix.Addr().IsUnspecified() || prefix.Addr().IsMulticast() {
			return errors.New("请输入有效的固定内网 IPv4 地址，例如 10.144.0.2/16")
		}
	}
	if opts.PeerID != "" && opts.PeerID != "none" && !uuidPattern.MatchString(opts.PeerID) {
		return errors.New("接入节点选择无效")
	}
	if opts.PublicAddress != "" {
		// Accept only a host, never a URL, path, credential or manually supplied port.
		host := strings.Trim(opts.PublicAddress, "[]")
		if addr, err := netip.ParseAddr(host); err == nil {
			if addr.IsUnspecified() || addr.IsMulticast() {
				return errors.New("接入地址无效")
			}
			opts.PublicAddress = addr.String()
		} else {
			if len(host) > 253 || strings.ContainsAny(host, "/:@?# \\ \t\n\r") {
				return errors.New("接入地址只填写域名或 IP，无需协议和端口")
			}
			for _, label := range strings.Split(host, ".") {
				if len(label) == 0 || len(label) > 63 || label[0] == '-' || label[len(label)-1] == '-' {
					return errors.New("接入地址无效")
				}
				for _, c := range label {
					if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-') {
						return errors.New("接入地址无效")
					}
				}
			}
		}
	}
	return nil
}
func initializeNetwork(ctx context.Context, db *sql.DB) error {
	bytes := make([]byte, 32)
	if _, err := rand.Read(bytes); err != nil {
		return err
	}
	tx, err := db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	result, err := tx.ExecContext(ctx, "INSERT IGNORE INTO managed_network (id,secret) VALUES (1,?)", hex.EncodeToString(bytes))
	if err != nil {
		return err
	}
	if n, _ := result.RowsAffected(); n > 0 {
		if _, err = tx.ExecContext(ctx, "UPDATE devices SET revision=revision+1 WHERE status='approved'"); err != nil {
			return err
		}
	}
	return tx.Commit()
}
func listenerPort(config string) string {
	// Preserve existing installations' internal port. New devices use 11010.
	var doc map[string]any
	if toml.Unmarshal([]byte(config), &doc) == nil {
		if values, ok := doc["listeners"].([]any); ok {
			for _, value := range values {
				text, ok := value.(string)
				if !ok {
					continue
				}
				u, err := url.Parse(text)
				if err != nil {
					continue
				}
				if n, err := strconv.Atoi(u.Port()); err == nil && n > 0 && n <= 65535 {
					return u.Port()
				}
			}
		}
	}
	return "11010"
}

type candidate struct {
	ID       string
	Config   string
	Settings DeviceSettings
	Address  string
}

func (s *server) candidates(ctx context.Context) ([]candidate, error) {
	rows, err := s.db.QueryContext(ctx, "SELECT d.id,d.config,COALESCE(p.preferences,''),COALESCE(p.observed_address,'') FROM devices d LEFT JOIN device_preferences p ON p.device_id=d.id WHERE d.status='approved' ORDER BY d.id")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []candidate{}
	for rows.Next() {
		var c candidate
		var pref string
		if err = rows.Scan(&c.ID, &c.Config, &pref, &c.Address); err != nil {
			return nil, err
		}
		c.Settings, err = settingsFromConfig(c.Config, pref)
		if err != nil {
			return nil, err
		}
		out = append(out, c)
	}
	return out, rows.Err()
}
func buildConfig(base string, opts DeviceSettings, secret string, peers []string) (string, error) {
	doc := map[string]any{}
	if base != "" {
		if err := toml.Unmarshal([]byte(base), &doc); err != nil {
			return "", errors.New("设备原有设置无法读取")
		}
	}
	for _, key := range []string{"core", "console_logger", "file_logger", "credential_file", "peer", "network_identity"} {
		delete(doc, key)
	}
	doc["network_identity"] = map[string]any{"network_name": "managed", "network_secret": secret}
	doc["dhcp"] = opts.AutoIP
	delete(doc, "ipv4")
	if !opts.AutoIP {
		doc["ipv4"] = opts.IPv4
	}
	delete(doc, "hostname")
	if opts.Hostname != "" {
		doc["hostname"] = opts.Hostname
	}
	listeners := []string{}
	if opts.AllowInbound {
		port := listenerPort(base)
		listeners = []string{"tcp://0.0.0.0:" + port, "udp://0.0.0.0:" + port}
	}
	doc["listeners"] = listeners
	flags, ok := doc["flags"].(map[string]any)
	if !ok {
		flags = map[string]any{}
	}
	flags["enable_ipv6"] = opts.EnableIPv6
	flags["disable_p2p"] = !opts.DirectConnection
	flags["disable_udp_hole_punching"] = !opts.UDPHolePunch
	doc["flags"] = flags
	if len(peers) > 0 {
		entries := []map[string]string{}
		for _, peer := range peers {
			entries = append(entries, map[string]string{"uri": peer})
		}
		doc["peer"] = entries
	}
	encoded, err := toml.Marshal(doc)
	return string(encoded), err
}
func (s *server) generatedConfig(ctx context.Context, id, base, pref string) (string, error) {
	opts, err := settingsFromConfig(base, pref)
	if err != nil {
		return "", err
	}
	var secret string
	if err = s.db.QueryRowContext(ctx, "SELECT secret FROM managed_network WHERE id=1").Scan(&secret); err != nil {
		return "", err
	}
	peers := []string{}
	if opts.PeerID != "none" {
		candidates, err := s.candidates(ctx)
		if err != nil {
			return "", err
		}
		seen := map[string]bool{}
		for _, c := range candidates {
			if c.ID == id || !c.Settings.AllowInbound || (opts.PeerID != "" && opts.PeerID != c.ID) {
				continue
			}
			host := c.Settings.PublicAddress
			if host == "" {
				host = c.Address
			}
			if host == "" {
				continue
			}
			endpoint := "tcp://" + net.JoinHostPort(host, listenerPort(c.Config))
			if !seen[endpoint] {
				peers = append(peers, endpoint)
				seen[endpoint] = true
			}
		}
	}
	return buildConfig(base, opts, secret, peers)
}
func (s *server) recordAddress(ctx context.Context, id, remote string) error {
	host, _, err := net.SplitHostPort(remote)
	if err != nil {
		return err
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	var status string
	if err = tx.QueryRowContext(ctx, "SELECT status FROM devices WHERE id=? FOR UPDATE", id).Scan(&status); err != nil {
		return err
	}
	var old string
	err = tx.QueryRowContext(ctx, "SELECT observed_address FROM device_preferences WHERE device_id=? FOR UPDATE", id).Scan(&old)
	if err != nil && !errors.Is(err, sql.ErrNoRows) {
		return err
	}
	if _, err = tx.ExecContext(ctx, "INSERT INTO device_preferences (device_id,preferences,observed_address) VALUES (?,'',?) ON DUPLICATE KEY UPDATE observed_address=VALUES(observed_address)", id, host); err != nil {
		return err
	}
	if old != host && status == "approved" {
		if _, err = tx.ExecContext(ctx, "UPDATE devices SET revision=revision+1 WHERE status='approved'"); err != nil {
			return err
		}
	}
	return tx.Commit()
}
