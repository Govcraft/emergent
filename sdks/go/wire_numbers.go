package emergent

import "math"

// wireUint64 converts a decoded wire number to a uint64.
//
// The two wire formats decode numbers differently. JSON yields float64 for
// every number. MessagePack yields the narrowest integer kind that holds the
// value, so the same field can arrive as int8, uint16, uint32 or uint64
// depending on its size. A type assertion on one kind therefore misses the
// others without any error.
//
// It reports false for a negative number, a float that is not finite or is too
// large, and anything that is not a number. A float's fraction is truncated.
func wireUint64(value any) (uint64, bool) {
	switch n := value.(type) {
	case uint64:
		return n, true
	case uint32:
		return uint64(n), true
	case uint16:
		return uint64(n), true
	case uint8:
		return uint64(n), true
	case uint:
		return uint64(n), true
	case int64:
		return nonNegativeToUint64(n)
	case int32:
		return nonNegativeToUint64(int64(n))
	case int16:
		return nonNegativeToUint64(int64(n))
	case int8:
		return nonNegativeToUint64(int64(n))
	case int:
		return nonNegativeToUint64(int64(n))
	case float64:
		return floatToUint64(n)
	case float32:
		return floatToUint64(float64(n))
	default:
		return 0, false
	}
}

func nonNegativeToUint64(n int64) (uint64, bool) {
	if n < 0 {
		return 0, false
	}
	return uint64(n), true
}

func floatToUint64(f float64) (uint64, bool) {
	// 2^64 is the first float64 above the uint64 range. NaN compares false
	// against everything, so it needs its own check.
	if math.IsNaN(f) || f < 0 || f >= 1<<64 {
		return 0, false
	}
	return uint64(f), true
}

// wireUint32 converts a decoded wire number to a uint32, reporting false when
// it does not fit. Process IDs travel this way.
func wireUint32(value any) (uint32, bool) {
	n, ok := wireUint64(value)
	if !ok || n > math.MaxUint32 {
		return 0, false
	}
	return uint32(n), true
}
