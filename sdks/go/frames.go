package emergent

// defaultErrorText stands in for the error text of an ERROR frame that
// carries none.
const defaultErrorText = "Engine returned an error"

// responseFromFrame reads the response a RESPONSE or ERROR frame carries.
//
// The engine answers a failed request with an ERROR frame whose body is the
// same response object a RESPONSE frame carries: correlation_id, success,
// error and error_code. An ERROR frame always reads as a failure here,
// whatever its success field says, and always has error text. It reports false
// for any other frame type and for a body with no string correlation_id, since
// nothing can be matched to it.
func responseFromFrame(msgType byte, payload any) (*IpcResponse, bool) {
	if msgType != MsgTypeResponse && msgType != MsgTypeError {
		return nil, false
	}
	body, ok := payload.(map[string]any)
	if !ok {
		return nil, false
	}
	correlationID, _ := body["correlation_id"].(string)
	if correlationID == "" {
		return nil, false
	}

	resp := &IpcResponse{CorrelationID: correlationID, Payload: body["payload"]}
	resp.Success, _ = body["success"].(bool)
	resp.Error, _ = body["error"].(string)
	resp.ErrorCode, _ = body["error_code"].(string)

	if msgType == MsgTypeError {
		resp.Success = false
		if resp.Error == "" {
			resp.Error = defaultErrorText
		}
	}
	return resp, true
}

// errorFrameText describes an ERROR frame body for a log line: the error
// text, followed by the error code in parentheses when the engine sent one.
// It falls back to defaultErrorText for a body that holds no error text.
func errorFrameText(payload any) string {
	body, ok := payload.(map[string]any)
	if !ok {
		return defaultErrorText
	}
	text, _ := body["error"].(string)
	if text == "" {
		text = defaultErrorText
	}
	if code, _ := body["error_code"].(string); code != "" {
		return text + " (" + code + ")"
	}
	return text
}

// frameCorrelationID returns the correlation id of a frame body for a log
// line, or "unknown" when the body has none.
func frameCorrelationID(payload any) string {
	body, _ := payload.(map[string]any)
	if id, _ := body["correlation_id"].(string); id != "" {
		return id
	}
	return "unknown"
}
