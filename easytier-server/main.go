package main

import (
	"context"
	"database/sql"
	"embed"
	"errors"
	"fmt"
	"io/fs"
	"log"
	"net/http"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/go-sql-driver/mysql"
)

//go:embed schema.sql web/*
var assets embed.FS

func openDB(ctx context.Context, dsn string) (*sql.DB, error) {
	cfg, err := mysql.ParseDSN(dsn)
	if err != nil {
		return nil, errors.New("invalid MYSQL_DSN")
	}
	cfg.ParseTime = true
	cfg.Loc = time.UTC
	cfg.Timeout, cfg.ReadTimeout, cfg.WriteTimeout = 5*time.Second, 10*time.Second, 10*time.Second
	cfg.MultiStatements = false
	connector, err := mysql.NewConnector(cfg)
	if err != nil {
		return nil, errors.New("invalid MySQL connection configuration")
	}
	db := sql.OpenDB(connector)
	db.SetMaxOpenConns(16)
	db.SetMaxIdleConns(4)
	db.SetConnMaxLifetime(3 * time.Minute)
	if err = db.PingContext(ctx); err != nil {
		db.Close()
		return nil, fmt.Errorf("connect MySQL: %w", err)
	}
	schema, _ := assets.ReadFile("schema.sql")
	for _, statement := range strings.Split(string(schema), ";") {
		if strings.TrimSpace(statement) == "" {
			continue
		}
		if _, err = db.ExecContext(ctx, statement); err != nil {
			db.Close()
			return nil, fmt.Errorf("initialize schema: %w", err)
		}
	}
	if err = initializeNetwork(ctx, db); err != nil {
		db.Close()
		return nil, fmt.Errorf("initialize managed network: %w", err)
	}
	return db, nil
}

func run() error {
	admin, enroll := os.Getenv("ADMIN_PASSWORD"), os.Getenv("ENROLLMENT_TOKEN")
	if len(admin) < 24 || len(enroll) < 24 || admin == enroll {
		return errors.New("set distinct ADMIN_PASSWORD and ENROLLMENT_TOKEN (at least 24 characters each)")
	}
	if os.Getenv("MYSQL_DSN") == "" {
		return errors.New("MYSQL_DSN is required")
	}
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	db, err := openDB(ctx, os.Getenv("MYSQL_DSN"))
	if err != nil {
		return err
	}
	defer db.Close()
	app := newServer(db, admin, enroll, os.Getenv("COOKIE_SECURE") != "false")
	static, _ := fs.Sub(assets, "web")
	handler := app.routes(http.FileServer(http.FS(static)))
	addr := os.Getenv("LISTEN_ADDR")
	if addr == "" {
		addr = "127.0.0.1:8080"
	}
	srv := &http.Server{Addr: addr, Handler: handler, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 15 * time.Second, WriteTimeout: 20 * time.Second, IdleTimeout: 60 * time.Second, MaxHeaderBytes: 16 << 10}
	days := 7
	if v := os.Getenv("LOG_RETENTION_DAYS"); v != "" {
		days, err = strconv.Atoi(v)
		if err != nil || days < 1 {
			return errors.New("invalid LOG_RETENTION_DAYS")
		}
	}
	go func() {
		ticker := time.NewTicker(time.Minute)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				// Bounded deletes keep log ingestion responsive on larger installations.
				if _, err := db.ExecContext(ctx, "DELETE FROM device_logs WHERE received_at < ? LIMIT 10000", time.Now().UTC().AddDate(0, 0, -days)); err != nil {
					log.Printf("log retention: %v", err)
				}
				app.expireSessions()
			}
		}
	}()
	go func() {
		<-ctx.Done()
		shutdown, done := context.WithTimeout(context.Background(), 10*time.Second)
		defer done()
		_ = srv.Shutdown(shutdown)
	}()
	log.Printf("EasyTier management listening on %s", addr)
	if err := srv.ListenAndServe(); !errors.Is(err, http.ErrServerClosed) {
		return err
	}
	return nil
}
func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}
