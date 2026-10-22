package artifact

import (
	"crypto/sha256"
	"encoding/hex"
	"strings"
	"testing"
)

func TestEmbeddedCoreMatchesProvenance(t *testing.T) {
	if len(strings.TrimSuffix(Commit, "-dirty")) != 40 {
		t.Fatalf("EasyTier commit = %q", Commit)
	}
	digest := sha256.Sum256(Core())
	if got := hex.EncodeToString(digest[:]); got != SHA256 {
		t.Fatalf("embedded core SHA-256 = %s, want %s", got, SHA256)
	}
}
