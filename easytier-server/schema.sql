CREATE TABLE IF NOT EXISTS devices (
 id VARCHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
 secret_hash BINARY(32) NOT NULL,
 hostname VARCHAR(255) NOT NULL,
 platform VARCHAR(64) NOT NULL,
 version VARCHAR(128) NOT NULL,
 status VARCHAR(16) NOT NULL DEFAULT 'pending',
 config MEDIUMTEXT NOT NULL,
 revision BIGINT NOT NULL DEFAULT 1,
 last_seen DATETIME(3) NOT NULL,
 registered_at DATETIME(3) NOT NULL,
 applied_revision BIGINT NOT NULL DEFAULT 0,
 runtime_status VARCHAR(32) NOT NULL DEFAULT 'waiting',
 runtime_error TEXT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
CREATE TABLE IF NOT EXISTS device_logs (
 id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
 device_id VARCHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
 event_id VARCHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
 timestamp VARCHAR(40) NOT NULL,
 level VARCHAR(8) NOT NULL,
 target VARCHAR(255) NOT NULL,
 message TEXT NOT NULL,
 received_at DATETIME(3) NOT NULL,
 UNIQUE KEY device_event (device_id, event_id),
 KEY device_cursor (device_id, id),
 KEY retention (received_at),
 CONSTRAINT logs_device FOREIGN KEY (device_id) REFERENCES devices(id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
CREATE TABLE IF NOT EXISTS managed_network (
 id TINYINT PRIMARY KEY,
 secret CHAR(64) CHARACTER SET ascii NOT NULL
) ENGINE=InnoDB;
CREATE TABLE IF NOT EXISTS device_preferences (
 device_id VARCHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
 preferences TEXT NOT NULL,
 observed_address VARCHAR(255) NOT NULL,
 CONSTRAINT preferences_device FOREIGN KEY (device_id) REFERENCES devices(id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
