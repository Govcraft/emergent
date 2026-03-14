package emergent

import (
	"context"
	"fmt"
	"net"
	"os"
	"strings"
	"sync"
	"time"
)

const (
	defaultTimeout    = 30 * time.Second
	emergentSocketEnv = "EMERGENT_SOCKET"
	emergentNameEnv   = "EMERGENT_NAME"
	readBufferSize    = 65536
	channelBufferSize = 256
)

// GetSocketPath returns the engine socket path from the EMERGENT_SOCKET environment variable.
func GetSocketPath() (string, error) {
	path := os.Getenv(emergentSocketEnv)
	if path == "" {
		return "", &ConnectionError{
			Msg: "EMERGENT_SOCKET environment variable not set. Make sure the Emergent engine is running.",
		}
	}
	return path, nil
}

// SocketExists checks if a socket file exists at the given path.
func SocketExists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

// pendingRequest tracks a request awaiting a response.
type pendingRequest struct {
	ch    chan ipcResponseResult
	timer *time.Timer
}

type ipcResponseResult struct {
	response *IpcResponse
	err      error
}

// pendingPubSubRequest tracks a pub/sub request awaiting a response.
type pendingPubSubRequest struct {
	ch    chan any
	timer *time.Timer
}

// baseClient provides shared connection logic for all primitive types.
type baseClient struct {
	name          string
	primitiveKind PrimitiveKind
	conn          net.Conn
	format        byte
	timeout       time.Duration
	logger        *Logger

	mu                          sync.Mutex
	disposed                    bool
	readBuffer                  []byte
	pendingRequests             map[string]*pendingRequest
	pendingTopologyRequests     map[string]*pendingPubSubRequest
	pendingSubscriptionRequests map[string]*pendingPubSubRequest
	messageStream               *MessageStream
	subscribedTypes             map[string]struct{}

	writeMu    sync.Mutex
	readCancel context.CancelFunc
	readDone   chan struct{}
}

func newBaseClient(name string, kind PrimitiveKind, opts *ConnectOptions) *baseClient {
	timeout := defaultTimeout
	if opts != nil && opts.Timeout > 0 {
		timeout = opts.Timeout
	}

	return &baseClient{
		name:                        name,
		primitiveKind:               kind,
		format:                      FormatMsgPack,
		timeout:                     timeout,
		logger:                      newLogger(name),
		pendingRequests:             make(map[string]*pendingRequest),
		pendingTopologyRequests:     make(map[string]*pendingPubSubRequest),
		pendingSubscriptionRequests: make(map[string]*pendingPubSubRequest),
		subscribedTypes:             make(map[string]struct{}),
		readDone:                    make(chan struct{}),
	}
}

func (c *baseClient) connect(opts *ConnectOptions) error {
	c.mu.Lock()
	defer c.mu.Unlock()

	if c.disposed {
		return &DisposedError{ClientType: string(c.primitiveKind)}
	}
	if c.conn != nil {
		return nil // Already connected
	}

	var socketPath string
	if opts != nil && opts.SocketPath != "" {
		socketPath = opts.SocketPath
	} else {
		var err error
		socketPath, err = GetSocketPath()
		if err != nil {
			return err
		}
	}

	c.logger.Info("connecting to engine", "kind", c.primitiveKind, "path", socketPath)

	if !SocketExists(socketPath) {
		c.logger.Error("engine socket not found", "path", socketPath)
		return &SocketNotFoundError{Path: socketPath}
	}

	conn, err := net.DialTimeout("unix", socketPath, c.timeout)
	if err != nil {
		c.logger.Error("failed to connect to engine", "error", err)
		return &ConnectionError{Msg: fmt.Sprintf("failed to connect to %s: %v", socketPath, err)}
	}

	c.conn = conn
	c.startReadLoop()

	c.logger.Info("connected to engine", "kind", c.primitiveKind)
	return nil
}

func (c *baseClient) subscribedTypesList() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	types := make([]string, 0, len(c.subscribedTypes))
	for t := range c.subscribedTypes {
		types = append(types, t)
	}
	return types
}

func (c *baseClient) subscribeInternal(ctx context.Context, messageTypes []string) (*MessageStream, error) {
	c.mu.Lock()
	if c.disposed {
		c.mu.Unlock()
		return nil, &DisposedError{ClientType: string(c.primitiveKind)}
	}
	if c.conn == nil {
		c.mu.Unlock()
		return nil, &ConnectionError{Msg: "not connected"}
	}
	c.mu.Unlock()

	c.logger.Info("subscribing to message types", "types", messageTypes)

	correlationID := generateCorrelationID("sub")

	stream := newMessageStream(channelBufferSize, func() {
		c.mu.Lock()
		if c.messageStream != nil {
			c.messageStream = nil
		}
		c.mu.Unlock()
	})

	c.mu.Lock()
	c.messageStream = stream
	c.mu.Unlock()

	// Add system.shutdown to subscriptions (SDK handles internally)
	allTypes := make([]string, len(messageTypes))
	copy(allTypes, messageTypes)
	hasShutdown := false
	for _, t := range allTypes {
		if t == "system.shutdown" {
			hasShutdown = true
			break
		}
	}
	if !hasShutdown {
		allTypes = append(allTypes, "system.shutdown")
	}

	resp, err := c.sendRequest(ctx, MsgTypeSubscribe, &IpcSubscribeRequest{
		CorrelationID: correlationID,
		MessageTypes:  allTypes,
	}, correlationID)
	if err != nil {
		stream.Close()
		return nil, &SubscriptionError{Msg: err.Error(), MessageTypes: messageTypes}
	}
	if !resp.Success {
		stream.Close()
		errMsg := resp.Error
		if errMsg == "" {
			errMsg = "subscription failed"
		}
		c.logger.Error("subscription failed", "types", messageTypes, "error", errMsg)
		return nil, &SubscriptionError{Msg: errMsg, MessageTypes: messageTypes}
	}

	// Track subscribed types (exclude internal system.shutdown)
	c.mu.Lock()
	for _, t := range messageTypes {
		if t != "system.shutdown" {
			c.subscribedTypes[t] = struct{}{}
		}
	}
	c.mu.Unlock()

	c.logger.Info("subscribed to message types", "types", messageTypes)
	return stream, nil
}

func (c *baseClient) unsubscribeInternal(ctx context.Context, messageTypes []string) error {
	c.mu.Lock()
	if c.disposed {
		c.mu.Unlock()
		return &DisposedError{ClientType: string(c.primitiveKind)}
	}
	if c.conn == nil {
		c.mu.Unlock()
		return &ConnectionError{Msg: "not connected"}
	}
	c.mu.Unlock()

	c.logger.Debug("unsubscribing from message types", "types", messageTypes)

	correlationID := generateCorrelationID("sub")
	resp, err := c.sendRequest(ctx, MsgTypeUnsubscribe, &IpcSubscribeRequest{
		CorrelationID: correlationID,
		MessageTypes:  messageTypes,
	}, correlationID)
	if err != nil {
		c.logger.Warn("unsubscribe failed", "types", messageTypes, "error", err)
		return nil // Best effort
	}
	if !resp.Success {
		c.logger.Warn("unsubscribe failed", "types", messageTypes, "error", resp.Error)
	}

	c.mu.Lock()
	for _, t := range messageTypes {
		delete(c.subscribedTypes, t)
	}
	c.mu.Unlock()

	return nil
}

func (c *baseClient) publishInternal(message *EmergentMessage) error {
	c.mu.Lock()
	if c.disposed {
		c.mu.Unlock()
		return &DisposedError{ClientType: string(c.primitiveKind)}
	}
	if c.conn == nil {
		c.mu.Unlock()
		return &ConnectionError{Msg: "not connected"}
	}
	c.mu.Unlock()

	// Set source to this client's name
	wireMsg := message.ToWire()
	wireMsg["source"] = c.name

	// Wrap in IPC envelope (fire-and-forget)
	envelope := &IpcEnvelope{
		CorrelationID: generateCorrelationID("pub"),
		Target:        "message_broker",
		MessageType:   "EmergentMessage",
		Payload:       map[string]any{"inner": wireMsg},
		ExpectsReply:  false,
	}

	frame, err := EncodeFrame(MsgTypeRequest, envelope, c.format)
	if err != nil {
		return &PublishError{Msg: fmt.Sprintf("encode error: %v", err), MessageType: string(message.MessageType)}
	}

	c.writeMu.Lock()
	_, err = c.conn.Write(frame)
	c.writeMu.Unlock()

	if err != nil {
		c.logger.Error("failed to publish message", "message_type", message.MessageType, "error", err)
		return &PublishError{Msg: err.Error(), MessageType: string(message.MessageType)}
	}

	c.logger.Debug("published message", "message_type", message.MessageType, "id", message.ID)
	return nil
}

func (c *baseClient) discoverInternal(ctx context.Context) (*DiscoveryInfo, error) {
	c.mu.Lock()
	if c.disposed {
		c.mu.Unlock()
		return nil, &DisposedError{ClientType: string(c.primitiveKind)}
	}
	if c.conn == nil {
		c.mu.Unlock()
		return nil, &ConnectionError{Msg: "not connected"}
	}
	c.mu.Unlock()

	c.logger.Debug("sending discovery request")

	correlationID := generateCorrelationID("req")

	envelope := &IpcEnvelope{
		CorrelationID: correlationID,
		Target:        "broker",
		MessageType:   "Discover",
		Payload:       nil,
		ExpectsReply:  true,
	}

	resp, err := c.sendRequest(ctx, MsgTypeDiscover, envelope, correlationID)
	if err != nil {
		return nil, &DiscoveryError{Msg: err.Error()}
	}
	if !resp.Success {
		errMsg := resp.Error
		if errMsg == "" {
			errMsg = "discovery failed"
		}
		return nil, &DiscoveryError{Msg: errMsg}
	}

	// Parse discovery response from payload
	info := &DiscoveryInfo{}
	payloadMap, ok := resp.Payload.(map[string]any)
	if ok {
		if types, ok := payloadMap["message_types"].([]any); ok {
			for _, t := range types {
				if s, ok := t.(string); ok {
					info.MessageTypes = append(info.MessageTypes, s)
				}
			}
		}
		if primitives, ok := payloadMap["primitives"].([]any); ok {
			for _, p := range primitives {
				if pm, ok := p.(map[string]any); ok {
					pi := PrimitiveInfo{}
					if name, ok := pm["name"].(string); ok {
						pi.Name = name
					}
					if kind, ok := pm["kind"].(string); ok {
						pi.Kind = PrimitiveKind(kind)
					}
					info.Primitives = append(info.Primitives, pi)
				}
			}
		}
	}

	c.logger.Debug("discovery complete", "message_types", len(info.MessageTypes), "primitives", len(info.Primitives))
	return info, nil
}

func (c *baseClient) getMySubscriptionsInternal(ctx context.Context) ([]string, error) {
	c.mu.Lock()
	if c.disposed {
		c.mu.Unlock()
		return nil, &DisposedError{ClientType: string(c.primitiveKind)}
	}
	if c.conn == nil {
		c.mu.Unlock()
		return nil, &ConnectionError{Msg: "not connected"}
	}
	c.mu.Unlock()

	c.logger.Debug("querying configured subscriptions")

	correlationID := generateCorrelationID("cor")

	// Subscribe to response type first
	subCorrelationID := generateCorrelationID("sub")
	subResp, err := c.sendRequest(ctx, MsgTypeSubscribe, &IpcSubscribeRequest{
		CorrelationID: subCorrelationID,
		MessageTypes:  []string{"system.response.subscriptions"},
	}, subCorrelationID)
	if err != nil {
		return nil, &ConnectionError{Msg: fmt.Sprintf("failed to subscribe to response type: %v", err)}
	}
	if !subResp.Success {
		return nil, &ConnectionError{Msg: subResp.Error}
	}

	// Create channel to wait for response
	resultCh := make(chan any, 1)
	timer := time.AfterFunc(c.timeout, func() {
		c.mu.Lock()
		delete(c.pendingSubscriptionRequests, correlationID)
		c.mu.Unlock()
		select {
		case resultCh <- &TimeoutError{Msg: "GetSubscriptions request timed out", Dur: c.timeout}:
		default:
		}
	})

	c.mu.Lock()
	c.pendingSubscriptionRequests[correlationID] = &pendingPubSubRequest{ch: resultCh, timer: timer}
	c.mu.Unlock()

	// Publish request message
	reqMsg, err := NewMessage("system.request.subscriptions")
	if err != nil {
		timer.Stop()
		c.mu.Lock()
		delete(c.pendingSubscriptionRequests, correlationID)
		c.mu.Unlock()
		return nil, err
	}
	reqMsg.WithCorrelationID(correlationID).WithPayload(map[string]any{"name": c.name})
	if pubErr := c.publishInternal(reqMsg); pubErr != nil {
		timer.Stop()
		c.mu.Lock()
		delete(c.pendingSubscriptionRequests, correlationID)
		c.mu.Unlock()
		return nil, pubErr
	}

	// Wait for response
	result := <-resultCh
	timer.Stop()

	switch v := result.(type) {
	case []string:
		c.logger.Info("received configured subscriptions", "types", v)
		return v, nil
	case error:
		return nil, v
	default:
		return nil, &ConnectionError{Msg: "unexpected response type"}
	}
}

func (c *baseClient) getTopologyInternal(ctx context.Context) (*TopologyState, error) {
	c.mu.Lock()
	if c.disposed {
		c.mu.Unlock()
		return nil, &DisposedError{ClientType: string(c.primitiveKind)}
	}
	if c.conn == nil {
		c.mu.Unlock()
		return nil, &ConnectionError{Msg: "not connected"}
	}
	c.mu.Unlock()

	c.logger.Debug("querying topology")

	correlationID := generateCorrelationID("cor")

	// Subscribe to response type first
	subCorrelationID := generateCorrelationID("sub")
	subResp, err := c.sendRequest(ctx, MsgTypeSubscribe, &IpcSubscribeRequest{
		CorrelationID: subCorrelationID,
		MessageTypes:  []string{"system.response.topology"},
	}, subCorrelationID)
	if err != nil {
		return nil, &ConnectionError{Msg: fmt.Sprintf("failed to subscribe to response type: %v", err)}
	}
	if !subResp.Success {
		return nil, &ConnectionError{Msg: subResp.Error}
	}

	// Create channel to wait for response
	resultCh := make(chan any, 1)
	timer := time.AfterFunc(c.timeout, func() {
		c.mu.Lock()
		delete(c.pendingTopologyRequests, correlationID)
		c.mu.Unlock()
		select {
		case resultCh <- &TimeoutError{Msg: "GetTopology request timed out", Dur: c.timeout}:
		default:
		}
	})

	c.mu.Lock()
	c.pendingTopologyRequests[correlationID] = &pendingPubSubRequest{ch: resultCh, timer: timer}
	c.mu.Unlock()

	// Publish request message
	reqMsg, err := NewMessage("system.request.topology")
	if err != nil {
		timer.Stop()
		c.mu.Lock()
		delete(c.pendingTopologyRequests, correlationID)
		c.mu.Unlock()
		return nil, err
	}
	reqMsg.WithCorrelationID(correlationID).WithPayload(map[string]any{})
	if pubErr := c.publishInternal(reqMsg); pubErr != nil {
		timer.Stop()
		c.mu.Lock()
		delete(c.pendingTopologyRequests, correlationID)
		c.mu.Unlock()
		return nil, pubErr
	}

	// Wait for response
	result := <-resultCh
	timer.Stop()

	switch v := result.(type) {
	case *TopologyState:
		c.logger.Debug("received topology", "primitive_count", len(v.Primitives))
		return v, nil
	case error:
		return nil, v
	default:
		return nil, &ConnectionError{Msg: "unexpected response type"}
	}
}

func (c *baseClient) close() error {
	c.mu.Lock()
	if c.disposed {
		c.mu.Unlock()
		return nil
	}

	c.logger.Info("disconnecting from engine", "kind", c.primitiveKind)

	// Cancel read loop
	if c.readCancel != nil {
		c.readCancel()
	}

	// Close message stream
	if c.messageStream != nil {
		c.messageStream.Close()
		c.messageStream = nil
	}

	// Close connection
	if c.conn != nil {
		c.conn.Close()
		c.conn = nil
	}

	// Cancel pending requests
	for id, pending := range c.pendingRequests {
		if pending.timer != nil {
			pending.timer.Stop()
		}
		select {
		case pending.ch <- ipcResponseResult{err: &ConnectionError{Msg: "connection closed"}}:
		default:
		}
		delete(c.pendingRequests, id)
	}

	// Cancel pending topology requests
	for id, pending := range c.pendingTopologyRequests {
		if pending.timer != nil {
			pending.timer.Stop()
		}
		select {
		case pending.ch <- &ConnectionError{Msg: "connection closed"}:
		default:
		}
		delete(c.pendingTopologyRequests, id)
	}

	// Cancel pending subscription requests
	for id, pending := range c.pendingSubscriptionRequests {
		if pending.timer != nil {
			pending.timer.Stop()
		}
		select {
		case pending.ch <- &ConnectionError{Msg: "connection closed"}:
		default:
		}
		delete(c.pendingSubscriptionRequests, id)
	}

	c.subscribedTypes = make(map[string]struct{})
	c.disposed = true
	c.mu.Unlock()

	// Wait for read loop to finish
	select {
	case <-c.readDone:
	case <-time.After(2 * time.Second):
	}

	c.logger.Info("disconnected from engine")
	c.logger.Close()
	return nil
}

func (c *baseClient) startReadLoop() {
	ctx, cancel := context.WithCancel(context.Background())
	c.readCancel = cancel

	go c.readLoop(ctx)
}

func (c *baseClient) readLoop(ctx context.Context) {
	defer close(c.readDone)

	c.logger.Debug("read loop started")
	buf := make([]byte, readBufferSize)

	for {
		select {
		case <-ctx.Done():
			return
		default:
		}

		// Set a read deadline so we can check context cancellation
		c.mu.Lock()
		conn := c.conn
		c.mu.Unlock()
		if conn == nil {
			return
		}

		if err := conn.SetReadDeadline(time.Now().Add(500 * time.Millisecond)); err != nil {
			return
		}

		n, err := conn.Read(buf)
		if err != nil {
			if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
				continue // Read deadline expired, check context
			}
			if ctx.Err() != nil {
				return // Context cancelled
			}
			c.logger.Info("connection closed (EOF)")
			// Close the message stream on EOF
			c.mu.Lock()
			if c.messageStream != nil {
				c.messageStream.Close()
				c.messageStream = nil
			}
			c.mu.Unlock()
			return
		}

		c.mu.Lock()
		c.readBuffer = append(c.readBuffer, buf[:n]...)
		c.mu.Unlock()

		c.processFrames()
	}
}

func (c *baseClient) processFrames() {
	c.mu.Lock()
	defer c.mu.Unlock()

	for len(c.readBuffer) >= HeaderSize {
		frame, err := TryDecodeFrame(c.readBuffer)
		if err != nil {
			c.logger.Error("protocol error while processing frame", "error", err)
			c.readBuffer = c.readBuffer[:0] // Reset buffer
			return
		}
		if frame == nil {
			return // Not enough data
		}

		c.readBuffer = c.readBuffer[frame.BytesConsumed:]
		c.handleFrame(frame.MsgType, frame.Payload)
	}
}

func (c *baseClient) handleFrame(msgType byte, payload any) {
	// Must be called with c.mu held
	switch msgType {
	case MsgTypeResponse:
		c.handleResponse(payload)
	case MsgTypePush:
		c.handlePush(payload)
	default:
		// Ignore unhandled message types (drain)
	}
}

func (c *baseClient) handleResponse(payload any) {
	payloadMap, ok := payload.(map[string]any)
	if !ok {
		return
	}

	correlationID, _ := payloadMap["correlation_id"].(string)
	if correlationID == "" {
		return
	}

	pending, exists := c.pendingRequests[correlationID]
	if !exists {
		return // No pending request — drain
	}

	delete(c.pendingRequests, correlationID)
	if pending.timer != nil {
		pending.timer.Stop()
	}

	resp := &IpcResponse{
		CorrelationID: correlationID,
	}
	if success, ok := payloadMap["success"].(bool); ok {
		resp.Success = success
	}
	resp.Payload = payloadMap["payload"]
	if errStr, ok := payloadMap["error"].(string); ok {
		resp.Error = errStr
	}
	if code, ok := payloadMap["error_code"].(string); ok {
		resp.ErrorCode = code
	}

	select {
	case pending.ch <- ipcResponseResult{response: resp}:
	default:
	}
}

func (c *baseClient) handlePush(payload any) {
	payloadMap, ok := payload.(map[string]any)
	if !ok {
		return
	}

	messageType, _ := payloadMap["message_type"].(string)

	// Handle system.shutdown internally
	if messageType == "system.shutdown" {
		c.handleShutdown(payloadMap)
		return
	}

	// Handle system.response.topology
	if messageType == "system.response.topology" {
		c.handleTopologyResponse(payloadMap)
		return
	}

	// Handle system.response.subscriptions
	if messageType == "system.response.subscriptions" {
		c.handleSubscriptionsResponse(payloadMap)
		return
	}

	// Forward to message stream
	if c.messageStream != nil {
		wirePayload, ok := payloadMap["payload"].(map[string]any)
		if ok {
			msg := MessageFromWire(wirePayload)
			c.logger.Debug("received message", "message_type", msg.MessageType, "source", msg.Source)
			c.messageStream.push(msg)
		}
	}
}

func (c *baseClient) handleShutdown(payloadMap map[string]any) {
	shutdownPayload, ok := payloadMap["payload"].(map[string]any)
	if !ok {
		return
	}

	shutdownKind, _ := shutdownPayload["kind"].(string)
	c.logger.Info("received shutdown signal", "kind", shutdownKind)

	if strings.EqualFold(shutdownKind, string(c.primitiveKind)) {
		c.logger.Info("shutting down (engine requested)")
		if c.messageStream != nil {
			c.messageStream.Close()
			c.messageStream = nil
		}
	}
}

func (c *baseClient) handleTopologyResponse(payloadMap map[string]any) {
	wirePayload, ok := payloadMap["payload"].(map[string]any)
	if !ok {
		return
	}

	correlationID, _ := wirePayload["correlation_id"].(string)
	if correlationID == "" {
		return
	}

	pending, exists := c.pendingTopologyRequests[correlationID]
	if !exists {
		return
	}
	delete(c.pendingTopologyRequests, correlationID)
	if pending.timer != nil {
		pending.timer.Stop()
	}

	// Extract primitives from the nested payload
	innerPayload, _ := wirePayload["payload"].(map[string]any)
	primitivesRaw, _ := innerPayload["primitives"].([]any)

	state := &TopologyState{}
	for _, pRaw := range primitivesRaw {
		if pm, ok := pRaw.(map[string]any); ok {
			tp := TopologyPrimitive{}
			tp.Name, _ = pm["name"].(string)
			tp.Kind, _ = pm["kind"].(string)
			tp.State, _ = pm["state"].(string)

			if pubs, ok := pm["publishes"].([]any); ok {
				for _, p := range pubs {
					if s, ok := p.(string); ok {
						tp.Publishes = append(tp.Publishes, s)
					}
				}
			}
			if subs, ok := pm["subscribes"].([]any); ok {
				for _, s := range subs {
					if str, ok := s.(string); ok {
						tp.Subscribes = append(tp.Subscribes, str)
					}
				}
			}
			if pid, ok := pm["pid"].(float64); ok {
				p := uint32(pid)
				tp.PID = &p
			}
			if errStr, ok := pm["error"].(string); ok {
				tp.Error = &errStr
			}

			state.Primitives = append(state.Primitives, tp)
		}
	}

	select {
	case pending.ch <- state:
	default:
	}
}

func (c *baseClient) handleSubscriptionsResponse(payloadMap map[string]any) {
	wirePayload, ok := payloadMap["payload"].(map[string]any)
	if !ok {
		return
	}

	correlationID, _ := wirePayload["correlation_id"].(string)
	if correlationID == "" {
		return
	}

	pending, exists := c.pendingSubscriptionRequests[correlationID]
	if !exists {
		return
	}
	delete(c.pendingSubscriptionRequests, correlationID)
	if pending.timer != nil {
		pending.timer.Stop()
	}

	// Extract subscribes list from nested payload
	innerPayload, _ := wirePayload["payload"].(map[string]any)
	subscribesRaw, _ := innerPayload["subscribes"].([]any)

	var result []string
	for _, s := range subscribesRaw {
		if str, ok := s.(string); ok {
			result = append(result, str)
		}
	}

	select {
	case pending.ch <- result:
	default:
	}
}

func (c *baseClient) sendRequest(ctx context.Context, msgType byte, payload any, correlationID string) (*IpcResponse, error) {
	resultCh := make(chan ipcResponseResult, 1)

	timer := time.AfterFunc(c.timeout, func() {
		c.mu.Lock()
		if _, exists := c.pendingRequests[correlationID]; exists {
			delete(c.pendingRequests, correlationID)
			c.mu.Unlock()
			resultCh <- ipcResponseResult{err: &TimeoutError{Msg: "request timed out", Dur: c.timeout}}
		} else {
			c.mu.Unlock()
		}
	})

	c.mu.Lock()
	c.pendingRequests[correlationID] = &pendingRequest{ch: resultCh, timer: timer}
	c.mu.Unlock()

	frame, err := EncodeFrame(msgType, payload, c.format)
	if err != nil {
		timer.Stop()
		c.mu.Lock()
		delete(c.pendingRequests, correlationID)
		c.mu.Unlock()
		return nil, fmt.Errorf("encode error: %w", err)
	}

	c.writeMu.Lock()
	_, err = c.conn.Write(frame)
	c.writeMu.Unlock()

	if err != nil {
		timer.Stop()
		c.mu.Lock()
		delete(c.pendingRequests, correlationID)
		c.mu.Unlock()
		return nil, &ConnectionError{Msg: fmt.Sprintf("failed to send: %v", err)}
	}

	select {
	case <-ctx.Done():
		timer.Stop()
		c.mu.Lock()
		delete(c.pendingRequests, correlationID)
		c.mu.Unlock()
		return nil, ctx.Err()
	case result := <-resultCh:
		timer.Stop()
		return result.response, result.err
	}
}

// resolveName resolves the primitive name from the argument or EMERGENT_NAME env var.
func resolveName(name, defaultName string) string {
	if name != "" {
		return name
	}
	if envName := os.Getenv(emergentNameEnv); envName != "" {
		return envName
	}
	return defaultName
}

