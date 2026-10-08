package fixture
import "testing"
func TestCounts(t *testing.T) {
    for _, input := range []string{"no", "", "-1", "999999999999999999999999"} { if _, err := ParseCount(input); err == nil { t.Fatalf("missing error: %q", input) } }
    for _, input := range []string{"0", "12"} { n, err := ParseCount(input); if err != nil || (input == "12" && n != 12) { t.Fatalf("valid: %d %v", n, err) } }
}
