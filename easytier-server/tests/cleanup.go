// Standalone helper used by e2e.py to remove only its own devices.
package main

import (
	"database/sql"
	"fmt"
	_ "github.com/go-sql-driver/mysql"
	"log"
	"os"
	"strconv"
)

func main() {
	db, err := sql.Open("mysql", os.Getenv("MYSQL_TEST_DSN"))
	if err != nil {
		log.Fatal(err)
	}
	defer db.Close()
	// Test-only legacy fixture: two clients share this host and must avoid TUN/port conflicts.
	if len(os.Args) == 4 && os.Args[1] == "--prepare" {
		port, err := strconv.Atoi(os.Args[3])
		if err != nil || port < 1 || port > 65535 {
			log.Fatal("invalid test port")
		}
		config := fmt.Sprintf("dhcp=true\nlisteners=['tcp://127.0.0.1:%d']\nstun_servers=[]\n[flags]\nno_tun=true\n", port)
		if _, err = db.Exec("UPDATE devices SET config=? WHERE id=?", config, os.Args[2]); err != nil {
			log.Fatal(err)
		}
		return
	}

	for _, id := range os.Args[1:] {
		if _, err = db.Exec("DELETE FROM device_preferences WHERE device_id=?", id); err != nil {
			log.Fatal(err)
		}
		if _, err = db.Exec("DELETE FROM device_logs WHERE device_id=?", id); err != nil {
			log.Fatal(err)
		}
		if _, err = db.Exec("DELETE FROM devices WHERE id=?", id); err != nil {
			log.Fatal(err)
		}
	}
}
