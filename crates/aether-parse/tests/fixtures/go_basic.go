package store

import (
	"context"
	"fmt"
	str "strings"

	"example.com/app/internal/db"
	dbx "example.com/app/internal/dbextra"
	"github.com/google/uuid"
	webappv1 "example.com/app/api/v1"
)

// Item is a stored value.
type Item struct {
	Key   string
	Value string
}

// Getter reads items.
type Getter interface {
	Get(ctx context.Context, key string) (*Item, error)
}

type (
	Celsius float64
	ItemMap = map[string]*Item
)

type (
	Kelvin float64
)

// Store keeps items in memory.
type Store[K comparable] struct {
	conn  *db.Conn
	items map[K]*Item
}

func NewStore(conn *db.Conn) *Store[string] {
	return &Store[string]{conn: conn, items: make(map[string]*Item)}
}

func (s *Store[K]) Get(ctx context.Context, key K) (*Item, error) {
	item, ok := s.items[key]
	if !ok {
		return nil, fmt.Errorf("missing %v", key)
	}
	s.touch(item)
	return item, nil
}

func (s *Store[K]) touch(item *Item) {
	type local struct{}
	item.Value = str.TrimSpace(item.Value)
	s.conn.Exec(item.Key)
	normalize(item)
}

func normalize(item *Item) {
	db.Flush()
	item.Key = uuid.New().String() + string(rune(max(len(item.Key), 1)))
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}

func Load(g Getter, conn *db.Conn, extra *dbx.Pool) Celsius {
	item, _ := g.Get(context.Background(), "k")
	conn.Ping()
	extra.Drain()
	return Celsius(len(item.Value))
}

func Map[T any](values []T, apply func(T) T) []T {
	return values
}

func Transform(values []int, next func(int) int) []int {
	apply := next
	apply(1)
	next(2)
	uuid := pick()
	uuid.Reset()
	return Map[int](values, next)
}

func Render(w *webappv1.Widget) {
	w.Render()
	webappv1.Register(w)
}

func _() {}
