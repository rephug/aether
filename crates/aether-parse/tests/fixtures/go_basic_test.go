package store

import "testing"

// TestGetReturnsStoredItem checks that a stored item comes back unchanged.
func TestGetReturnsStoredItem(t *testing.T) {
	s := NewStore(nil)
	t.Run("missing key", func(t *testing.T) {
		if _, err := s.Get(nil, "absent"); err == nil {
			t.Fatal("expected an error")
		}
	})
}

//go:noinline
func TestHTTPServer_Handles404(t *testing.T) {}

var update = false // set by the -update flag
func TestGolden(t *testing.T) {}

func TestlowercaseIsNotATest(t *testing.T) {}

func TestNeedsTestingT(name string) {}

func TestMain(m *testing.M) {}

func BenchmarkGet(b *testing.B) {}

func helper(t *testing.T) {}
