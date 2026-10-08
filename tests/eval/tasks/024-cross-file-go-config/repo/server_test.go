package fixture
import "testing"
func TestPort(t *testing.T) { if Port() != 8080 { t.Fatal("default port") } }
