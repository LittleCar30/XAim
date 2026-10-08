package fixture
import "testing"
func TestNames(t *testing.T) { if UserName(" Alice ") != "alice" || TeamName(" Team ") != "team" { t.Fatal("bad normalization") } }
