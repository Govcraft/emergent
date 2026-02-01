/**
 * Emergent Topology Viewer - D3.js Force-Directed Graph
 */

// State
let nodes = [];
let edges = [];
let simulation = null;
let svg = null;
let g = null;
let linkGroup = null;
let nodeGroup = null;

// D3 selections
let linkSelection = null;
let nodeSelection = null;

// Color mappings
const kindColors = {
  source: "source",
  handler: "handler",
  sink: "sink",
};

const statusColors = {
  running: null, // use kind color
  stopped: "stopped",
  error: "error",
};

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

  // Define arrow marker
  svg
    .append("defs")
    .append("marker")
    .attr("id", "arrow")
    .attr("viewBox", "0 -5 10 10")
    .attr("refX", 35)
    .attr("refY", 0)
    .attr("markerWidth", 6)
    .attr("markerHeight", 6)
    .attr("orient", "auto")
    .append("path")
    .attr("d", "M0,-5L10,0L0,5")
    .attr("fill", "#4a5568");

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

  // Initialize force simulation
  simulation = d3
    .forceSimulation()
    .force(
      "link",
      d3
        .forceLink()
        .id((d) => d.id)
        .distance(150)
    )
    .force("charge", d3.forceManyBody().strength(-400))
    .force("center", d3.forceCenter(width / 2, height / 2))
    .force("collision", d3.forceCollide().radius(40))
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
 * Get the CSS class for a node based on its status and kind.
 */
function getNodeClass(node) {
  if (node.status === "stopped") return "stopped";
  if (node.status === "error") return "error";
  return kindColors[node.kind] || "handler";
}

/**
 * Update the graph with new data.
 */
function updateGraph() {
  // Update node count
  document.getElementById("node-count").textContent = `${nodes.length} nodes`;

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

  linkEnter.transition().duration(300).attr("stroke-opacity", 0.6);

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
    .attr("stroke", "#1a1a2e")
    .attr("stroke-opacity", 0.5);

  nodeEnter
    .append("text")
    .attr("dy", "0.35em")
    .text((d) => d.id);

  nodeEnter.transition().duration(300).attr("opacity", 1);

  // Update existing nodes
  nodeSelection
    .select("circle")
    .attr("class", (d) => getNodeClass(d));

  nodeSelection = nodeEnter.merge(nodeSelection);

  // Update all node circles with current class
  nodeSelection.select("circle").attr("class", (d) => getNodeClass(d));

  // Add event handlers for tooltips
  nodeSelection
    .on("mouseenter", showTooltip)
    .on("mousemove", moveTooltip)
    .on("mouseleave", hideTooltip);

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
      const dx = d.target.x - d.source.x;
      const dy = d.target.y - d.source.y;
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

/**
 * Helper to create a text element and append it to a container.
 */
function appendTextElement(container, className, text) {
  const el = document.createElement("div");
  if (className) el.className = className;
  el.textContent = text;
  container.appendChild(el);
  return el;
}

/**
 * Show tooltip for a node using safe DOM manipulation.
 */
function showTooltip(event, d) {
  const tooltip = document.getElementById("tooltip");

  // Clear existing content safely
  while (tooltip.firstChild) {
    tooltip.removeChild(tooltip.firstChild);
  }

  // Title
  appendTextElement(tooltip, "tooltip-title", d.id);

  // Kind and status
  appendTextElement(tooltip, "tooltip-kind", `${d.kind} (${d.status})`);

  // PID if present
  if (d.pid) {
    appendTextElement(tooltip, "", `PID: ${d.pid}`);
  }

  // Publishes section
  if (d.publishes && d.publishes.length > 0) {
    const section = document.createElement("div");
    section.className = "tooltip-section";
    appendTextElement(section, "tooltip-section-title", "Publishes");
    const list = document.createElement("div");
    list.className = "tooltip-list";
    d.publishes.forEach((p) => {
      appendTextElement(list, "", p);
    });
    section.appendChild(list);
    tooltip.appendChild(section);
  }

  // Subscribes section
  if (d.subscribes && d.subscribes.length > 0) {
    const section = document.createElement("div");
    section.className = "tooltip-section";
    appendTextElement(section, "tooltip-section-title", "Subscribes");
    const list = document.createElement("div");
    list.className = "tooltip-list";
    d.subscribes.forEach((s) => {
      appendTextElement(list, "", s);
    });
    section.appendChild(list);
    tooltip.appendChild(section);
  }

  // Error if present
  if (d.error) {
    appendTextElement(tooltip, "tooltip-error", d.error);
  }

  tooltip.classList.add("visible");
  moveTooltip(event);
}

/**
 * Move tooltip to follow mouse.
 */
function moveTooltip(event) {
  const tooltip = document.getElementById("tooltip");
  const padding = 15;
  let x = event.clientX + padding;
  let y = event.clientY + padding;

  // Keep tooltip on screen
  const rect = tooltip.getBoundingClientRect();
  if (x + rect.width > window.innerWidth) {
    x = event.clientX - rect.width - padding;
  }
  if (y + rect.height > window.innerHeight) {
    y = event.clientY - rect.height - padding;
  }

  tooltip.style.left = `${x}px`;
  tooltip.style.top = `${y}px`;
}

/**
 * Hide tooltip.
 */
function hideTooltip() {
  const tooltip = document.getElementById("tooltip");
  tooltip.classList.remove("visible");
}

/**
 * Connect to SSE endpoint with auto-reconnect.
 */
function connectSSE() {
  const statusEl = document.getElementById("connection-status");

  const eventSource = new EventSource("/events");

  eventSource.onopen = () => {
    console.log("[SSE] Connected");
    statusEl.textContent = "Connected";
    statusEl.className = "connected";
  };

  eventSource.onerror = () => {
    console.log("[SSE] Connection error, reconnecting...");
    statusEl.textContent = "Reconnecting...";
    statusEl.className = "disconnected";
  };

  // Handle full topology update
  eventSource.addEventListener("topology:full", (event) => {
    const state = JSON.parse(event.data);
    console.log("[SSE] topology:full", state);

    // Preserve existing node positions
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

    // Check if node already exists (shouldn't happen, but be safe)
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
      // Preserve position
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
}

/**
 * Manually refresh the topology from the API.
 */
async function refreshTopology() {
  try {
    const response = await fetch("/api/topology");
    if (!response.ok) {
      console.error("[Refresh] Failed to fetch topology:", response.statusText);
      return;
    }

    const state = await response.json();
    console.log("[Refresh] topology", state);

    // Preserve existing node positions
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
    console.error("[Refresh] Error:", err);
  }
}

// Initialize on page load
document.addEventListener("DOMContentLoaded", () => {
  initGraph();
  connectSSE();

  // Wire up refresh button
  const refreshBtn = document.getElementById("refresh-btn");
  if (refreshBtn) {
    refreshBtn.addEventListener("click", refreshTopology);
  }
});
