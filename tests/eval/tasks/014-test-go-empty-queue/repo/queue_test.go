package fixture
import "testing"
func TestOne(t *testing.T) { q := &Queue{}; q.Push(7); value, ok := q.Pop(); if value != 7 || !ok { t.Fatal("wrong value") } }
