package fixture
import "testing"
func TestFirst(t *testing.T) {
    for _, values := range [][]int{nil, {}, {0}, {4, 5}} {
        got, ok := First(values)
        if len(values) == 0 { if got != 0 || ok { t.Fatalf("empty: %d %v", got, ok) } } else if got != values[0] || !ok { t.Fatal("wrong first value") }
    }
}
