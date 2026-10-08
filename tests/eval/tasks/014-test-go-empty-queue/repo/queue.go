package fixture
type Queue struct { items []int }
func (q *Queue) Push(value int) { q.items = append(q.items, value) }
func (q *Queue) Pop() (int, bool) {
    if len(q.items) == 0 { return 0, false }
    result := q.items[0]
    q.items = q.items[1:]
    return result, true
}
