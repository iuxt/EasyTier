package host

import (
	"crypto/rand"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"github.com/easytier/easytier/easytier-go/proto/common"
	"sort"
	"strings"
	"sync"
)

type managedInstance struct {
	id       *common.UUID
	instance *Instance
}
type instanceManager struct {
	mu        sync.RWMutex
	instances map[string]*managedInstance
}

func newInstanceManager() *instanceManager {
	return &instanceManager{instances: make(map[string]*managedInstance)}
}

func (manager *instanceManager) register(entry *managedInstance) error {
	id := uuidString(entry.id)
	manager.mu.Lock()
	defer manager.mu.Unlock()
	if _, exists := manager.instances[id]; exists {
		return fmt.Errorf("EasyTier instance %s already exists", id)
	}
	manager.instances[id] = entry
	return nil
}

func (manager *instanceManager) remove(instance *Instance) {
	manager.mu.Lock()
	if entry := manager.instances[instance.id]; entry != nil &&
		entry.instance == instance {
		delete(manager.instances, instance.id)
	}
	manager.mu.Unlock()
}

func (manager *instanceManager) clear() {
	manager.mu.Lock()
	manager.instances = make(map[string]*managedInstance)
	manager.mu.Unlock()
}

func (manager *instanceManager) snapshot() []*managedInstance {
	manager.mu.RLock()
	entries := make([]*managedInstance, 0, len(manager.instances))
	for _, entry := range manager.instances {
		snapshot := *entry
		entries = append(entries, &snapshot)
	}
	manager.mu.RUnlock()
	sort.Slice(entries, func(i, j int) bool {
		return uuidString(entries[i].id) < uuidString(entries[j].id)
	})
	return entries
}

func parseInstanceUUID(text string) (*common.UUID, string, error) {
	normalized := strings.ReplaceAll(strings.TrimSpace(text), "-", "")
	if len(normalized) != 32 {
		return nil, "", fmt.Errorf("invalid EasyTier instance ID %q", text)
	}
	var value [16]byte
	decoded, err := hex.DecodeString(normalized)
	if err != nil || len(decoded) != 16 {
		return nil, "", fmt.Errorf("invalid EasyTier instance ID %q", text)
	}
	copy(value[:], decoded)
	id := &common.UUID{
		Part1: binary.BigEndian.Uint32(value[0:4]),
		Part2: binary.BigEndian.Uint32(value[4:8]),
		Part3: binary.BigEndian.Uint32(value[8:12]),
		Part4: binary.BigEndian.Uint32(value[12:16]),
	}
	return id, uuidString(id), nil
}

func newInstanceUUID() (*common.UUID, string, error) {
	var value [16]byte
	if _, err := rand.Read(value[:]); err != nil {
		return nil, "", fmt.Errorf("generate EasyTier instance ID: %w", err)
	}
	value[6] = value[6]&0x0f | 0x40
	value[8] = value[8]&0x3f | 0x80
	id := &common.UUID{
		Part1: binary.BigEndian.Uint32(value[0:4]),
		Part2: binary.BigEndian.Uint32(value[4:8]),
		Part3: binary.BigEndian.Uint32(value[8:12]),
		Part4: binary.BigEndian.Uint32(value[12:16]),
	}
	return id, uuidString(id), nil
}

func uuidString(id *common.UUID) string {
	var value [16]byte
	binary.BigEndian.PutUint32(value[0:4], id.GetPart1())
	binary.BigEndian.PutUint32(value[4:8], id.GetPart2())
	binary.BigEndian.PutUint32(value[8:12], id.GetPart3())
	binary.BigEndian.PutUint32(value[12:16], id.GetPart4())
	return fmt.Sprintf(
		"%x-%x-%x-%x-%x",
		value[0:4],
		value[4:6],
		value[6:8],
		value[8:10],
		value[10:16],
	)
}

func bindInstanceIdentity(config, id, name string) string {
	var encoded strings.Builder
	writeTOMLStringField(&encoded, "instance_id", id)
	writeTOMLStringField(&encoded, "instance_name", name)
	encoded.WriteByte('\n')
	encoded.WriteString(config)
	return encoded.String()
}
