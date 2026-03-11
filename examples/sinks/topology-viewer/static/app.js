/**
 * Emergent Topology Viewer
 * D3.js Force-Directed Graph with Observatory UI
 */

// =============================================================================
// State
// =============================================================================
let nodes = [];
let edges = [];
let simulation = null;
let svg = null;
let g = null;
let linkGroup = null;
let nodeGroup = null;
let selectedNodeId = "emergent-engine";

// D3 selections
let linkSelection = null;
let nodeSelection = null;

// Color mappings by node kind
const kindColors = {
  source: "source",
  handler: "handler",
  sink: "sink",
};

// =============================================================================
// Initialization
// =============================================================================

/**
 * Initialize the graph SVG and force simulation.
 */
function initGraph() {
  const container = document.querySelector("main");
  const width = container.clientWidth;
  const height = container.clientHeight;

  svg = d3
    .select("#graph")
    .attr("width", width)
    .attr("height", height)
    .attr("viewBox", [0, 0, width, height]);

  // Define arrow marker with updated color
  const defs = svg.append("defs");

  defs.append("marker")
    .attr("id", "arrow")
    .attr("viewBox", "0 -5 10 10")
    .attr("refX", 35)
    .attr("refY", 0)
    .attr("markerWidth", 6)
    .attr("markerHeight", 6)
    .attr("orient", "auto")
    .append("path")
    .attr("d", "M0,-5L10,0L0,5")
    .attr("fill", "#3a3a50");

  // Add zoom behavior
  const zoom = d3
    .zoom()
    .scaleExtent([0.25, 4])
    .on("zoom", (event) => {
      g.attr("transform", event.transform);
    });

  svg.call(zoom);

  // Main group for zoom/pan
  g = svg.append("g");

  // Groups for links and nodes (links behind nodes)
  linkGroup = g.append("g").attr("class", "links");
  nodeGroup = g.append("g").attr("class", "nodes");

  // Initialize force simulation with easing parameters
  simulation = d3
    .forceSimulation()
    .force(
      "link",
      d3.forceLink()
        .id((d) => d.id)
        .distance(150)
    )
    .force("charge", d3.forceManyBody().strength(-400))
    .force("center", d3.forceCenter(width / 2, height / 2))
    .force("collision", d3.forceCollide().radius(40))
    .alphaDecay(0.02)
    .velocityDecay(0.4)
    .on("tick", ticked);

  // Handle window resize
  window.addEventListener("resize", () => {
    const newWidth = container.clientWidth;
    const newHeight = container.clientHeight;
    svg.attr("width", newWidth).attr("height", newHeight);
    simulation.force("center", d3.forceCenter(newWidth / 2, newHeight / 2));
    simulation.alpha(0.3).restart();
  });
}

/**
 * Get the CSS class for a node based on its kind.
 */
function getNodeClass(node) {
  return kindColors[node.kind] || "handler";
}

// =============================================================================
// Graph Updates
// =============================================================================

/**
 * Update the graph with new data.
 */
function updateGraph() {
  // Update node count display
  const nodeCountEl = document.getElementById("node-count");
  if (nodeCountEl) {
    // Clear and rebuild safely
    while (nodeCountEl.firstChild) {
      nodeCountEl.removeChild(nodeCountEl.firstChild);
    }
    const valueSpan = document.createElement("span");
    valueSpan.className = "stat-value";
    valueSpan.textContent = nodes.length;
    const labelSpan = document.createElement("span");
    labelSpan.className = "stat-label";
    labelSpan.textContent = "nodes";
    nodeCountEl.appendChild(valueSpan);
    nodeCountEl.appendChild(labelSpan);
  }

  // Hide canvas hint when we have nodes
  const canvasHint = document.getElementById("canvas-hint");
  if (canvasHint) {
    canvasHint.classList.toggle("hidden", nodes.length > 0);
  }

  // Create edge data with source/target references
  const edgeData = edges.map((e) => ({
    ...e,
    source: e.source,
    target: e.target,
  }));

  // Update links
  linkSelection = linkGroup.selectAll(".edge").data(edgeData, (d) => `${d.source}-${d.target}-${d.messageType}`);

  linkSelection.exit().transition().duration(300).attr("stroke-opacity", 0).remove();

  const linkEnter = linkSelection
    .enter()
    .append("path")
    .attr("class", "edge")
    .attr("marker-end", "url(#arrow)")
    .attr("stroke-opacity", 0);

  linkEnter.transition().duration(300).attr("stroke-opacity", 0.5);

  linkSelection = linkEnter.merge(linkSelection);

  // Update nodes
  nodeSelection = nodeGroup.selectAll(".node").data(nodes, (d) => d.id);

  nodeSelection.exit().transition().duration(300).attr("opacity", 0).remove();

  const nodeEnter = nodeSelection
    .enter()
    .append("g")
    .attr("class", "node")
    .attr("opacity", 0)
    .call(drag(simulation));

  nodeEnter
    .append("circle")
    .attr("r", 24)
    .attr("stroke", "#0a0a0f")
    .attr("stroke-width", 2);

  nodeEnter
    .append("text")
    .attr("dy", 42)
    .text((d) => d.id);

  nodeEnter.transition().duration(300).attr("opacity", 1);

  // Update existing nodes
  nodeSelection
    .select("circle")
    .attr("class", (d) => getNodeClass(d));

  nodeSelection = nodeEnter.merge(nodeSelection);

  // Update all node circles with current class
  nodeSelection.select("circle").attr("class", (d) => getNodeClass(d));

  // Add click handler for selection
  nodeSelection.on("click", selectNode);

  // Update selected node visual and details
  updateSelectedNodeVisual();

  // Update details panel for selected node
  const selectedNode = nodes.find((n) => n.id === selectedNodeId);
  if (selectedNode) {
    updateNodeDetails(selectedNode);
  }

  // Update depth-of-field effect
  updateDepthOfField();

  // Update simulation
  simulation.nodes(nodes);
  simulation.force("link").links(edgeData);
  simulation.alpha(0.5).restart();
}

/**
 * Update positions on each simulation tick.
 */
function ticked() {
  if (linkSelection) {
    linkSelection.attr("d", (d) => {
      return `M${d.source.x},${d.source.y}L${d.target.x},${d.target.y}`;
    });
  }

  if (nodeSelection) {
    nodeSelection.attr("transform", (d) => `translate(${d.x},${d.y})`);
  }
}

/**
 * Create drag behavior for nodes.
 */
function drag(simulation) {
  function dragstarted(event) {
    if (!event.active) simulation.alphaTarget(0.3).restart();
    event.subject.fx = event.subject.x;
    event.subject.fy = event.subject.y;
  }

  function dragged(event) {
    event.subject.fx = event.x;
    event.subject.fy = event.y;
  }

  function dragended(event) {
    if (!event.active) simulation.alphaTarget(0);
    event.subject.fx = null;
    event.subject.fy = null;
  }

  return d3
    .drag()
    .on("start", dragstarted)
    .on("drag", dragged)
    .on("end", dragended);
}

// =============================================================================
// Selection & Depth of Field
// =============================================================================

/**
 * Select a node.
 */
function selectNode(event, d) {
  selectedNodeId = d.id;
  updateSelectedNodeVisual();
  updateNodeDetails(d);
  updateDepthOfField();
}

/**
 * Get the set of neighbor node IDs for a given node.
 */
function getNeighborIds(nodeId) {
  const neighbors = new Set();
  edges.forEach((e) => {
    const sourceId = typeof e.source === "object" ? e.source.id : e.source;
    const targetId = typeof e.target === "object" ? e.target.id : e.target;
    if (sourceId === nodeId) {
      neighbors.add(targetId);
    } else if (targetId === nodeId) {
      neighbors.add(sourceId);
    }
  });
  return neighbors;
}

/**
 * Update the depth-of-field effect based on selection.
 */
function updateDepthOfField() {
  if (!nodeSelection || !linkSelection) return;

  const neighborIds = getNeighborIds(selectedNodeId);

  const isNodeForeground = (d) => d.id === selectedNodeId || neighborIds.has(d.id);

  const isEdgeForeground = (d) => {
    const sourceId = typeof d.source === "object" ? d.source.id : d.source;
    const targetId = typeof d.target === "object" ? d.target.id : d.target;
    return sourceId === selectedNodeId || targetId === selectedNodeId;
  };

  // Update node depth classes
  nodeSelection
    .classed("foreground", isNodeForeground)
    .classed("background", (d) => !isNodeForeground(d));

  // Scale circles and text for depth effect
  nodeSelection.select("circle")
    .transition()
    .duration(300)
    .attr("r", (d) => isNodeForeground(d) ? 24 : 16);

  nodeSelection.select("text")
    .transition()
    .duration(300)
    .attr("dy", (d) => isNodeForeground(d) ? 42 : 30)
    .style("font-size", (d) => isNodeForeground(d) ? "11px" : "9px");

  // Update edge depth classes
  linkSelection
    .classed("foreground", isEdgeForeground)
    .classed("background", (d) => !isEdgeForeground(d));

  // Reorder nodes so foreground nodes render on top
  nodeSelection.sort((a, b) => {
    const aForeground = isNodeForeground(a);
    const bForeground = isNodeForeground(b);
    if (aForeground && !bForeground) return 1;
    if (!aForeground && bForeground) return -1;
    if (a.id === selectedNodeId) return 1;
    if (b.id === selectedNodeId) return 1;
    return 0;
  });

  // Update force simulation for micro-interactions
  updateForceSimulation(neighborIds);
}

/**
 * Update force simulation for micro-interactions.
 */
function updateForceSimulation(neighborIds) {
  if (!simulation) return;

  const container = document.querySelector("main");
  const centerX = container.clientWidth / 2;
  const centerY = container.clientHeight / 2;

  // Phase 1: Brief spread
  simulation.force("charge").strength(-500);
  simulation.force("radial", null);

  simulation
    .alpha(0.3)
    .alphaTarget(0.1)
    .restart();

  // Phase 2: Gather/disperse
  setTimeout(() => {
    simulation.force("link")
      .distance((d) => {
        const sourceId = typeof d.source === "object" ? d.source.id : d.source;
        const targetId = typeof d.target === "object" ? d.target.id : d.target;
        const isForeground = sourceId === selectedNodeId || targetId === selectedNodeId;
        return isForeground ? 100 : 200;
      });

    simulation.force("charge")
      .strength((d) => {
        if (d.id === selectedNodeId) return -200;
        if (neighborIds.has(d.id)) return -150;
        return -600;
      });

    simulation.force("radial", d3.forceRadial((d) => {
      if (d.id === selectedNodeId) return 0;
      if (neighborIds.has(d.id)) return 120;
      return 350;
    }, centerX, centerY).strength((d) => {
      if (d.id === selectedNodeId) return 0.8;
      if (neighborIds.has(d.id)) return 0.15;
      return 0.1;
    }));

    simulation
      .alpha(0.4)
      .alphaTarget(0)
      .restart();
  }, 150);
}

/**
 * Update the visual appearance of the selected node.
 */
function updateSelectedNodeVisual() {
  if (!nodeSelection) return;

  nodeSelection.select("circle")
    .classed("selected", (d) => d.id === selectedNodeId);
}

// =============================================================================
// Inspector Panel
// =============================================================================

/**
 * Update the node details/inspector panel.
 */
function updateNodeDetails(node) {
  const panel = document.getElementById("node-details");
  const nameEl = document.getElementById("inspector-name");
  const kindEl = document.getElementById("inspector-kind");
  const iconEl = document.getElementById("inspector-icon");
  const inspector = document.getElementById("inspector");

  if (!panel || !nameEl || !kindEl || !iconEl) return;

  // Clear existing content
  while (panel.firstChild) {
    panel.removeChild(panel.firstChild);
  }

  if (!node) {
    nameEl.textContent = "—";
    kindEl.textContent = "—";
    kindEl.className = "inspector-badge";
    iconEl.className = "inspector-icon";
    inspector?.classList.add("hidden");

    const empty = document.createElement("div");
    empty.className = "inspector-empty";
    const emptyText = document.createElement("p");
    emptyText.textContent = "Select a node to view details";
    empty.appendChild(emptyText);
    panel.appendChild(empty);
    return;
  }

  // Show inspector
  inspector?.classList.remove("hidden");

  // Update header
  nameEl.textContent = node.id;
  kindEl.textContent = node.kind;
  kindEl.className = `inspector-badge ${node.kind}`;
  iconEl.className = `inspector-icon ${node.kind}`;

  // Process info section (only if we have useful data)
  if (node.pid) {
    const processSection = createSection("Process");
    const processContent = document.createElement("div");
    processContent.className = "inspector-section-content";

    const pidRow = createRow("PID", node.pid);
    processContent.appendChild(pidRow);

    processSection.appendChild(processContent);
    panel.appendChild(processSection);
  }

  // Publishes section
  if (node.publishes && node.publishes.length > 0) {
    const pubSection = createSection("Publishes");
    const list = document.createElement("div");
    list.className = "message-list";

    node.publishes.forEach((p) => {
      const item = document.createElement("div");
      item.className = "message-item pub";
      item.textContent = p;
      list.appendChild(item);
    });

    pubSection.appendChild(list);
    panel.appendChild(pubSection);
  }

  // Subscribes section
  if (node.subscribes && node.subscribes.length > 0) {
    const subSection = createSection("Subscribes");
    const list = document.createElement("div");
    list.className = "message-list";

    node.subscribes.forEach((s) => {
      const item = document.createElement("div");
      item.className = "message-item sub";
      item.textContent = s;
      list.appendChild(item);
    });

    subSection.appendChild(list);
    panel.appendChild(subSection);
  }

  // Error section
  if (node.error) {
    const errorSection = createSection("Error");
    const errorContent = document.createElement("div");
    errorContent.className = "inspector-section-content";
    errorContent.style.color = "var(--color-disconnected)";
    errorContent.textContent = node.error;
    errorSection.appendChild(errorContent);
    panel.appendChild(errorSection);
  }
}

/**
 * Create an inspector section.
 */
function createSection(title) {
  const section = document.createElement("div");
  section.className = "inspector-section";

  const titleEl = document.createElement("div");
  titleEl.className = "inspector-section-title";
  titleEl.textContent = title;
  section.appendChild(titleEl);

  return section;
}

/**
 * Create a key-value row.
 */
function createRow(label, value) {
  const row = document.createElement("div");
  row.className = "inspector-row";

  const labelEl = document.createElement("span");
  labelEl.className = "inspector-row-label";
  labelEl.textContent = label;

  const valueEl = document.createElement("span");
  valueEl.className = "inspector-row-value";
  valueEl.textContent = value;

  row.appendChild(labelEl);
  row.appendChild(valueEl);

  return row;
}

// =============================================================================
// SSE Connection
// =============================================================================

/**
 * Connect to SSE endpoint with auto-reconnect.
 */
function connectSSE() {
  const statusDot = document.getElementById("connection-dot");
  const statusText = document.getElementById("connection-status");

  const eventSource = new EventSource("/events");

  eventSource.onopen = () => {
    console.log("[SSE] Connected");
    if (statusDot) statusDot.classList.add("connected");
    if (statusText) statusText.textContent = "Connected";
  };

  eventSource.onerror = () => {
    console.log("[SSE] Connection error, reconnecting...");
    if (statusDot) statusDot.classList.remove("connected");
    if (statusText) statusText.textContent = "Reconnecting";
  };

  // Handle full topology update
  eventSource.addEventListener("topology:full", (event) => {
    const state = JSON.parse(event.data);
    console.log("[SSE] topology:full", state);

    const positionMap = new Map();
    nodes.forEach((n) => {
      if (n.x !== undefined) {
        positionMap.set(n.id, { x: n.x, y: n.y });
      }
    });

    nodes = state.nodes.map((n) => {
      const pos = positionMap.get(n.id);
      return pos ? { ...n, x: pos.x, y: pos.y } : n;
    });
    edges = state.edges;
    updateGraph();
  });

  // Handle node added
  eventSource.addEventListener("node:added", (event) => {
    const node = JSON.parse(event.data);
    console.log("[SSE] node:added", node);

    const existingIndex = nodes.findIndex((n) => n.id === node.id);
    if (existingIndex >= 0) {
      nodes[existingIndex] = { ...nodes[existingIndex], ...node };
    } else {
      nodes.push(node);
    }
    updateGraph();
  });

  // Handle node updated
  eventSource.addEventListener("node:updated", (event) => {
    const node = JSON.parse(event.data);
    console.log("[SSE] node:updated", node);

    const existingIndex = nodes.findIndex((n) => n.id === node.id);
    if (existingIndex >= 0) {
      const { x, y, vx, vy, fx, fy } = nodes[existingIndex];
      nodes[existingIndex] = { ...node, x, y, vx, vy, fx, fy };
    } else {
      nodes.push(node);
    }
    updateGraph();
  });

  // Handle edges updated
  eventSource.addEventListener("edges:updated", (event) => {
    const newEdges = JSON.parse(event.data);
    console.log("[SSE] edges:updated", newEdges);
    edges = newEdges;
    updateGraph();
  });

  // Handle message activity
  eventSource.addEventListener("message:activity", (event) => {
    const { source, messageType } = JSON.parse(event.data);
    console.log("[SSE] message:activity", source, "→", messageType);
    animateEdge(source, messageType);
  });
}

// =============================================================================
// Refresh & Animation
// =============================================================================

const TOPOLOGY_API_URL = "http://localhost:8892";

/**
 * Request a topology refresh.
 */
async function refreshTopology() {
  const refreshBtn = document.getElementById("refresh-btn");
  if (refreshBtn) {
    refreshBtn.classList.add("loading");
  }

  try {
    const response = await fetch(`${TOPOLOGY_API_URL}/refresh`);
    if (!response.ok) {
      console.error("[Refresh] Failed:", response.statusText);
      return;
    }

    const result = await response.json();
    console.log("[Refresh] Request sent:", result);
  } catch (err) {
    console.error("[Refresh] Error:", err);
    console.log("[Refresh] Falling back to local state");
    await refreshTopologyLocal();
  } finally {
    if (refreshBtn) {
      refreshBtn.classList.remove("loading");
    }
  }
}

/**
 * Fall back to local state.
 */
async function refreshTopologyLocal() {
  try {
    const response = await fetch("/api/topology");
    if (!response.ok) {
      console.error("[Refresh] Failed to fetch local topology:", response.statusText);
      return;
    }

    const state = await response.json();
    console.log("[Refresh] Local topology", state);

    const positionMap = new Map();
    nodes.forEach((n) => {
      if (n.x !== undefined) {
        positionMap.set(n.id, { x: n.x, y: n.y });
      }
    });

    nodes = state.nodes.map((n) => {
      const pos = positionMap.get(n.id);
      return pos ? { ...n, x: pos.x, y: pos.y } : n;
    });
    edges = state.edges;
    updateGraph();
  } catch (err) {
    console.error("[Refresh] Local error:", err);
  }
}

/**
 * Animate an edge to show message activity.
 */
function animateEdge(source, messageType) {
  if (!linkSelection) return;

  linkSelection
    .filter((d) => {
      if (d.source.id !== source && d.source !== source) return false;
      return d.messageType === messageType || matchesPattern(messageType, d.messageType);
    })
    .classed("active", true)
    .transition()
    .duration(300)
    .attr("stroke-opacity", 1)
    .transition()
    .duration(500)
    .attr("stroke-opacity", 0.5)
    .on("end", function () {
      d3.select(this).classed("active", false);
    });
}

/**
 * Check if a message type matches a pattern.
 */
function matchesPattern(messageType, pattern) {
  if (pattern === "*") return true;
  if (pattern === messageType) return true;
  if (!pattern.includes("*")) return false;

  const regex = new RegExp(
    "^" + pattern.replace(/\./g, "\\.").replace(/\*/g, "[^.]+") + "$"
  );
  return regex.test(messageType);
}

// =============================================================================
// Initialize
// =============================================================================

document.addEventListener("DOMContentLoaded", () => {
  initGraph();
  connectSSE();

  const refreshBtn = document.getElementById("refresh-btn");
  if (refreshBtn) {
    refreshBtn.addEventListener("click", refreshTopology);
  }
});
