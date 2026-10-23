package main

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"log"
	"net"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/go-sql-driver/mysql"
)

type session struct{ expires time.Time }
type attempt struct {
	count   int
	expires time.Time
}
type server struct {
	db                *sql.DB
	admin, enrollment [32]byte
	secure            bool
	mu                sync.Mutex
	sessions          map[string]session
	attempts          map[string]attempt
}

func newServer(db *sql.DB, admin, enrollment string, secure bool) *server {
	return &server{db: db, admin: sha256.Sum256([]byte(admin)), enrollment: sha256.Sum256([]byte(enrollment)), secure: secure, sessions: map[string]session{}, attempts: map[string]attempt{}}
}
func equalHash(value string, expected [32]byte) bool {
	got := sha256.Sum256([]byte(value))
	return subtle.ConstantTimeCompare(got[:], expected[:]) == 1
}
func bearer(r *http.Request) string {
	v := r.Header.Get("Authorization")
	if !strings.HasPrefix(v, "Bearer ") {
		return ""
	}
	return strings.TrimPrefix(v, "Bearer ")
}
func reply(w http.ResponseWriter, code int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(value)
}
func fail(w http.ResponseWriter, code int, message string) {
	reply(w, code, map[string]string{"error": message})
}
func decode(w http.ResponseWriter, r *http.Request, v any) bool {
	if !strings.HasPrefix(r.Header.Get("Content-Type"), "application/json") {
		fail(w, 415, "JSON required")
		return false
	}
	r.Body = http.MaxBytesReader(w, r.Body, 1<<20)
	d := json.NewDecoder(r.Body)
	d.DisallowUnknownFields()
	if err := d.Decode(v); err != nil {
		fail(w, 400, "invalid JSON")
		return false
	}
	if err := d.Decode(new(any)); err != io.EOF {
		fail(w, 400, "invalid JSON")
		return false
	}
	return true
}
func (s *server) dbError(w http.ResponseWriter, err error) {
	log.Printf("database operation: %v", err)
	fail(w, 500, "database operation failed")
}
func (s *server) routes(static http.Handler) http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /api/login", s.login)
	mux.HandleFunc("POST /api/logout", s.adminOnly(s.logout))
	mux.HandleFunc("GET /api/admin/devices", s.adminOnly(s.listDevices))
	mux.HandleFunc("PUT /api/admin/devices/{id}", s.adminOnly(s.updateDevice))
	mux.HandleFunc("GET /api/admin/devices/{id}/logs", s.adminOnly(s.logs))
	mux.HandleFunc("POST /api/client/register", s.register)
	mux.HandleFunc("POST /api/client/{id}/sync", s.syncDevice)
	mux.HandleFunc("POST /api/client/{id}/logs", s.ingestLogs)
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, r *http.Request) {
		if err := s.db.PingContext(r.Context()); err != nil {
			fail(w, 503, "MySQL unavailable")
			return
		}
		reply(w, 200, map[string]string{"status": "ok"})
	})
	mux.Handle("/", static)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Content-Type-Options", "nosniff")
		w.Header().Set("X-Frame-Options", "DENY")
		w.Header().Set("Referrer-Policy", "no-referrer")
		w.Header().Set("Cache-Control", "no-store")
		w.Header().Set("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")
		if r.Method != "GET" && r.Method != "HEAD" {
			// JSON-only endpoints plus same-origin checks protect cookie-authenticated writes.
			if origin := r.Header.Get("Origin"); origin != "" {
				u, err := url.Parse(origin)
				scheme := "https"
				if !s.secure {
					scheme = "http"
				}
				if err != nil || u.Host != r.Host || u.Scheme != scheme {
					fail(w, 403, "origin rejected")
					return
				}
			}
			if r.Header.Get("Sec-Fetch-Site") == "cross-site" {
				fail(w, 403, "origin rejected")
				return
			}
		}
		mux.ServeHTTP(w, r)
	})
}
func (s *server) expireSessions() {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := time.Now()
	for k, v := range s.sessions {
		if now.After(v.expires) {
			delete(s.sessions, k)
		}
	}
	for k, v := range s.attempts {
		if now.After(v.expires) {
			delete(s.attempts, k)
		}
	}
}
func (s *server) login(w http.ResponseWriter, r *http.Request) {
	host, _, _ := net.SplitHostPort(r.RemoteAddr)
	s.mu.Lock()
	a := s.attempts[host]
	if time.Now().After(a.expires) {
		a = attempt{expires: time.Now().Add(time.Minute)}
	}
	a.count++
	if len(s.attempts) < 4096 || s.attempts[host].count > 0 {
		s.attempts[host] = a
	} else {
		a.count = 11
	}
	s.mu.Unlock()
	if a.count > 10 {
		fail(w, 429, "too many attempts; retry in one minute")
		return
	}
	var in struct {
		Password string `json:"password"`
	}
	if !decode(w, r, &in) {
		return
	}
	if !equalHash(in.Password, s.admin) {
		fail(w, 401, "invalid password")
		return
	}
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		fail(w, 500, "session creation failed")
		return
	}
	token := hex.EncodeToString(b)
	s.mu.Lock()
	if len(s.sessions) >= 1024 {
		s.mu.Unlock()
		fail(w, 429, "too many sessions")
		return
	}
	s.sessions[token] = session{time.Now().Add(12 * time.Hour)}
	s.mu.Unlock()
	http.SetCookie(w, &http.Cookie{Name: "et_session", Value: token, Path: "/", HttpOnly: true, Secure: s.secure, SameSite: http.SameSiteStrictMode, MaxAge: 43200})
	reply(w, 200, map[string]bool{"ok": true})
}
func (s *server) adminOnly(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		c, err := r.Cookie("et_session")
		if err != nil {
			fail(w, 401, "login required")
			return
		}
		s.mu.Lock()
		v, ok := s.sessions[c.Value]
		s.mu.Unlock()
		if !ok || time.Now().After(v.expires) {
			fail(w, 401, "login expired")
			return
		}
		next(w, r)
	}
}
func (s *server) logout(w http.ResponseWriter, r *http.Request) {
	c, _ := r.Cookie("et_session")
	s.mu.Lock()
	delete(s.sessions, c.Value)
	s.mu.Unlock()
	http.SetCookie(w, &http.Cookie{Name: "et_session", Path: "/", MaxAge: -1, HttpOnly: true, Secure: s.secure, SameSite: http.SameSiteStrictMode})
	reply(w, 200, map[string]bool{"ok": true})
}

var uuidPattern = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)

type device struct {
	ID              string         `json:"id"`
	Hostname        string         `json:"hostname"`
	Platform        string         `json:"platform"`
	Version         string         `json:"version"`
	Status          string         `json:"status"`
	Config          string         `json:"-"`
	Settings        DeviceSettings `json:"settings"`
	Revision        int64          `json:"revision"`
	LastSeen        time.Time      `json:"last_seen"`
	RegisteredAt    time.Time      `json:"registered_at"`
	AppliedRevision int64          `json:"applied_revision"`
	RuntimeStatus   string         `json:"runtime_status"`
	RuntimeError    string         `json:"runtime_error"`
}

func (s *server) register(w http.ResponseWriter, r *http.Request) {
	if !equalHash(bearer(r), s.enrollment) {
		fail(w, 401, "invalid enrollment token")
		return
	}
	var in struct {
		ID       string `json:"id"`
		Secret   string `json:"secret"`
		Hostname string `json:"hostname"`
		Platform string `json:"platform"`
		Version  string `json:"version"`
	}
	if !decode(w, r, &in) {
		return
	}
	if !uuidPattern.MatchString(in.ID) || len(in.Secret) < 32 || len(in.Secret) > 256 || len(in.Hostname) > 255 || len(in.Platform) > 64 || len(in.Version) > 128 {
		fail(w, 400, "invalid device identity")
		return
	}
	hash := sha256.Sum256([]byte(in.Secret))
	now := time.Now().UTC()
	_, err := s.db.ExecContext(r.Context(), "INSERT INTO devices (id,secret_hash,hostname,platform,version,config,last_seen,registered_at,runtime_error) VALUES (?,?,?,?,?,'',?,?,'')", in.ID, hash[:], in.Hostname, in.Platform, in.Version, now, now)
	if err != nil {
		var mysqlErr *mysql.MySQLError
		if !errors.As(err, &mysqlErr) || mysqlErr.Number != 1062 {
			s.dbError(w, err)
			return
		}
		var stored []byte
		if err = s.db.QueryRowContext(r.Context(), "SELECT secret_hash FROM devices WHERE id=?", in.ID).Scan(&stored); err != nil {
			s.dbError(w, err)
			return
		}
		if subtle.ConstantTimeCompare(stored, hash[:]) != 1 {
			fail(w, 403, "device identity conflict")
			return
		}
		if _, err = s.db.ExecContext(r.Context(), "UPDATE devices SET hostname=?,platform=?,version=?,last_seen=? WHERE id=?", in.Hostname, in.Platform, in.Version, now, in.ID); err != nil {
			s.dbError(w, err)
			return
		}
	}
	if err := s.recordAddress(r.Context(), in.ID, r.RemoteAddr); err != nil {
		s.dbError(w, err)
		return
	}
	reply(w, 200, map[string]string{"status": "registered"})
}
func (s *server) authenticate(w http.ResponseWriter, r *http.Request) bool {
	if !uuidPattern.MatchString(r.PathValue("id")) {
		fail(w, 400, "invalid id")
		return false
	}
	var hash []byte
	err := s.db.QueryRowContext(r.Context(), "SELECT secret_hash FROM devices WHERE id=?", r.PathValue("id")).Scan(&hash)
	if errors.Is(err, sql.ErrNoRows) {
		fail(w, 404, "device not registered")
		return false
	}
	if err != nil {
		s.dbError(w, err)
		return false
	}
	got := sha256.Sum256([]byte(bearer(r)))
	if subtle.ConstantTimeCompare(got[:], hash) != 1 {
		fail(w, 401, "invalid device credentials")
		return false
	}
	return true
}
func (s *server) syncDevice(w http.ResponseWriter, r *http.Request) {
	if !s.authenticate(w, r) {
		return
	}
	var in struct {
		Applied int64  `json:"applied_revision"`
		State   string `json:"runtime_status"`
		Error   string `json:"runtime_error"`
	}
	if !decode(w, r, &in) {
		return
	}
	if len(in.State) > 32 || len(in.Error) > 8192 || in.Applied < 0 {
		fail(w, 400, "invalid runtime state")
		return
	}
	if _, err := s.db.ExecContext(r.Context(), "UPDATE devices SET last_seen=?,applied_revision=?,runtime_status=?,runtime_error=? WHERE id=?", time.Now().UTC(), in.Applied, in.State, in.Error, r.PathValue("id")); err != nil {
		s.dbError(w, err)
		return
	}
	if err := s.recordAddress(r.Context(), r.PathValue("id"), r.RemoteAddr); err != nil {
		s.dbError(w, err)
		return
	}
	var status, config, preferences string
	var revision int64
	if err := s.db.QueryRowContext(r.Context(), "SELECT d.status,d.config,d.revision,COALESCE(p.preferences,'') FROM devices d LEFT JOIN device_preferences p ON p.device_id=d.id WHERE d.id=?", r.PathValue("id")).Scan(&status, &config, &revision, &preferences); err != nil {
		s.dbError(w, err)
		return
	}
	if status != "approved" {
		config = ""
		revision = 0
	} else {
		generated, err := s.generatedConfig(r.Context(), r.PathValue("id"), config, preferences)
		if err != nil {
			s.dbError(w, err)
			return
		}
		config = generated
	}
	reply(w, 200, map[string]any{"status": status, "config": config, "revision": revision, "lease_seconds": 45})
}
func (s *server) listDevices(w http.ResponseWriter, r *http.Request) {
	rows, err := s.db.QueryContext(r.Context(), "SELECT d.id,d.hostname,d.platform,d.version,d.status,d.config,d.revision,d.last_seen,d.registered_at,d.applied_revision,d.runtime_status,d.runtime_error,COALESCE(p.preferences,'') FROM devices d LEFT JOIN device_preferences p ON p.device_id=d.id ORDER BY d.registered_at DESC")
	if err != nil {
		s.dbError(w, err)
		return
	}
	defer rows.Close()
	out := []device{}
	for rows.Next() {
		var d device
		var preferences string
		if err = rows.Scan(&d.ID, &d.Hostname, &d.Platform, &d.Version, &d.Status, &d.Config, &d.Revision, &d.LastSeen, &d.RegisteredAt, &d.AppliedRevision, &d.RuntimeStatus, &d.RuntimeError, &preferences); err != nil {
			s.dbError(w, err)
			return
		}
		d.Settings, err = settingsFromConfig(d.Config, preferences)
		if err != nil {
			s.dbError(w, err)
			return
		}
		out = append(out, d)
	}
	if err = rows.Err(); err != nil {
		s.dbError(w, err)
		return
	}
	reply(w, 200, out)
}
func (s *server) updateDevice(w http.ResponseWriter, r *http.Request) {
	var in struct {
		Status   string          `json:"status"`
		Settings *DeviceSettings `json:"settings"`
		Revision int64           `json:"revision"`
	}
	if !decode(w, r, &in) {
		return
	}
	switch in.Status {
	case "pending", "approved", "rejected", "revoked":
	default:
		fail(w, 400, "invalid status")
		return
	}
	if in.Settings == nil && in.Status == "approved" {
		fail(w, 400, "请提交设备设置")
		return
	}
	id := r.PathValue("id")
	if in.Settings != nil {
		if err := in.Settings.validate(); err != nil {
			fail(w, 400, err.Error())
			return
		}
		if in.Settings.PeerID == id {
			fail(w, 400, "不能选择设备自身作为接入节点")
			return
		}
		if peer := in.Settings.PeerID; peer != "" && peer != "none" {
			var base, pref string
			err := s.db.QueryRowContext(r.Context(), "SELECT d.config,COALESCE(p.preferences,'') FROM devices d LEFT JOIN device_preferences p ON p.device_id=d.id WHERE d.id=? AND d.status='approved'", peer).Scan(&base, &pref)
			if errors.Is(err, sql.ErrNoRows) {
				fail(w, 400, "请选择已批准的接入节点")
				return
			}
			if err != nil {
				s.dbError(w, err)
				return
			}
			options, err := settingsFromConfig(base, pref)
			if err != nil {
				s.dbError(w, err)
				return
			}
			if !options.AllowInbound {
				fail(w, 400, "所选设备已关闭接入，请重新选择")
				return
			}
		}
	}
	tx, err := s.db.BeginTx(r.Context(), nil)
	if err != nil {
		s.dbError(w, err)
		return
	}
	defer tx.Rollback()
	var base, secret, oldStatus, oldPreferences string
	if err = tx.QueryRowContext(r.Context(), "SELECT d.config,d.status,COALESCE(p.preferences,'') FROM devices d LEFT JOIN device_preferences p ON p.device_id=d.id WHERE d.id=?", id).Scan(&base, &oldStatus, &oldPreferences); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			fail(w, 404, "设备不存在")
		} else {
			s.dbError(w, err)
		}
		return
	}
	if err = tx.QueryRowContext(r.Context(), "SELECT secret FROM managed_network WHERE id=1").Scan(&secret); err != nil {
		s.dbError(w, err)
		return
	}
	config := base
	topologyChanged := (oldStatus == "approved") != (in.Status == "approved")
	if in.Settings != nil {
		oldOptions, err := settingsFromConfig(base, oldPreferences)
		if err != nil {
			fail(w, 400, err.Error())
			return
		}
		if (oldStatus == "approved" || in.Status == "approved") && (oldOptions.AllowInbound != in.Settings.AllowInbound || oldOptions.PublicAddress != in.Settings.PublicAddress) {
			topologyChanged = true
		}
		config, err = buildConfig(base, *in.Settings, secret, nil)
		if err != nil {
			fail(w, 400, err.Error())
			return
		}
	}
	result, err := tx.ExecContext(r.Context(), "UPDATE devices SET status=?,config=?,revision=revision+1 WHERE id=? AND revision=?", in.Status, config, id, in.Revision)
	if err != nil {
		s.dbError(w, err)
		return
	}
	if n, _ := result.RowsAffected(); n == 0 {
		fail(w, 409, "设备设置已变化，请刷新后重试")
		return
	}
	if in.Settings != nil {
		preferences, _ := json.Marshal(in.Settings)
		if _, err = tx.ExecContext(r.Context(), "INSERT INTO device_preferences (device_id,preferences,observed_address) VALUES (?,?,'') ON DUPLICATE KEY UPDATE preferences=VALUES(preferences)", id, preferences); err != nil {
			s.dbError(w, err)
			return
		}
	}
	if topologyChanged {
		if _, err = tx.ExecContext(r.Context(), "UPDATE devices SET revision=revision+1 WHERE id<>? AND status='approved'", id); err != nil {
			s.dbError(w, err)
			return
		}
	}
	if err = tx.Commit(); err != nil {
		s.dbError(w, err)
		return
	}
	reply(w, 200, map[string]bool{"ok": true})
}

type logEntry struct {
	ID        string `json:"id"`
	Timestamp string `json:"timestamp"`
	Level     string `json:"level"`
	Target    string `json:"target"`
	Message   string `json:"message"`
}

func (s *server) ingestLogs(w http.ResponseWriter, r *http.Request) {
	if !s.authenticate(w, r) {
		return
	}
	var in struct {
		Logs []logEntry `json:"logs"`
	}
	if !decode(w, r, &in) {
		return
	}
	if len(in.Logs) > 100 {
		fail(w, 400, "maximum 100 logs per batch")
		return
	}
	for _, e := range in.Logs {
		if !uuidPattern.MatchString(e.ID) || len(e.Timestamp) > 40 || len(e.Level) > 8 || len(e.Target) > 255 || len(e.Message) > 8192 {
			fail(w, 400, "invalid log entry")
			return
		}
	}
	tx, err := s.db.BeginTx(r.Context(), nil)
	if err != nil {
		s.dbError(w, err)
		return
	}
	defer tx.Rollback()
	for _, e := range in.Logs {
		_, err = tx.ExecContext(r.Context(), "INSERT INTO device_logs (device_id,event_id,timestamp,level,target,message,received_at) VALUES (?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE event_id=event_id", r.PathValue("id"), e.ID, e.Timestamp, e.Level, e.Target, e.Message, time.Now().UTC())
		if err != nil {
			s.dbError(w, err)
			return
		}
	}
	if err = tx.Commit(); err != nil {
		s.dbError(w, err)
		return
	}
	reply(w, 200, map[string]bool{"ok": true})
}
func (s *server) logs(w http.ResponseWriter, r *http.Request) {
	rows, err := s.db.QueryContext(r.Context(), "SELECT event_id,timestamp,level,target,message FROM device_logs WHERE device_id=? ORDER BY id DESC LIMIT 500", r.PathValue("id"))
	if err != nil {
		s.dbError(w, err)
		return
	}
	defer rows.Close()
	out := []logEntry{}
	for rows.Next() {
		var e logEntry
		if err = rows.Scan(&e.ID, &e.Timestamp, &e.Level, &e.Target, &e.Message); err != nil {
			s.dbError(w, err)
			return
		}
		out = append(out, e)
	}
	if err = rows.Err(); err != nil {
		s.dbError(w, err)
		return
	}
	reply(w, 200, out)
}
