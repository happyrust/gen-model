# Zone-Based XKT Chunking and View-Based Loading Solution

## 1. Architecture Overview

### 1.1 Data Structure
```
Database 1112
├── ZONE_001
│   ├── Equipment_A
│   ├── Equipment_B
│   └── Pipes
├── ZONE_002
│   ├── Equipment_C
│   └── Structures
└── ZONE_003
    └── ...
```

### 1.2 File Structure
```
output/zones/db1112/
├── zone_manifest.json       # Zone spatial index and metadata
├── zone_001.xkt             # Zone 001 geometry
├── zone_002.xkt             # Zone 002 geometry
├── zone_003.xkt             # Zone 003 geometry
└── ...
```

## 2. Implementation Components

### 2.1 Backend: Zone XKT Generation

#### Phase 1: Zone Discovery
- Query all zones in database 1112
- Get bounding box for each zone
- Get reference numbers for all elements in each zone

#### Phase 2: XKT Generation
- Generate individual XKT file for each zone
- Include zone metadata (name, bbox, element count)
- Optimize with compression for network transfer

### 2.2 Spatial Indexing

#### Zone Manifest Structure
```json
{
  "database": 1112,
  "totalZones": 25,
  "boundingBox": {
    "min": [-1000, -1000, -1000],
    "max": [1000, 1000, 1000]
  },
  "zones": [
    {
      "id": "zone_001",
      "name": "Process Area A",
      "refno": "17496/266203",
      "boundingBox": {
        "min": [-100, -100, 0],
        "max": [100, 100, 200]
      },
      "center": [0, 0, 100],
      "radius": 173.2,
      "elementCount": 1250,
      "fileSize": 245678,
      "xktFile": "zone_001.xkt",
      "compressed": true
    },
    {
      "id": "zone_002",
      "name": "Utility Area",
      "refno": "17497/256215",
      "boundingBox": {
        "min": [100, -100, 0],
        "max": [300, 100, 150]
      },
      "center": [200, 0, 75],
      "radius": 150.0,
      "elementCount": 890,
      "fileSize": 189234,
      "xktFile": "zone_002.xkt",
      "compressed": true
    }
  ]
}
```

### 2.3 Client-Side View-Based Loading

#### Loading Strategy
1. **Initial Load**: Load zones intersecting initial camera view
2. **Dynamic Loading**: Load/unload zones based on camera movement
3. **LOD Strategy**: Load detailed models for near zones, simplified for far zones
4. **Preloading**: Predictively load adjacent zones based on camera direction

#### View Frustum Culling Algorithm
```javascript
class ZoneLoader {
    constructor(camera, scene, manifest) {
        this.camera = camera;
        this.scene = scene;
        this.manifest = manifest;
        this.loadedZones = new Map();
        this.loadingQueue = [];
        this.viewDistance = 500; // meters
        this.preloadDistance = 700; // meters
    }

    updateVisibleZones() {
        const frustum = this.camera.getFrustum();
        const cameraPos = this.camera.position;

        // Find zones to load
        const zonesToLoad = this.manifest.zones.filter(zone => {
            // Check if zone bbox intersects frustum
            if (!frustum.intersectsBox(zone.boundingBox)) {
                return false;
            }

            // Check distance from camera
            const distance = this.getDistance(cameraPos, zone.center);
            return distance < this.viewDistance;
        });

        // Load new zones
        zonesToLoad.forEach(zone => {
            if (!this.loadedZones.has(zone.id)) {
                this.loadZone(zone);
            }
        });

        // Unload distant zones
        this.loadedZones.forEach((loadedZone, id) => {
            const zone = this.manifest.zones.find(z => z.id === id);
            const distance = this.getDistance(cameraPos, zone.center);

            if (distance > this.viewDistance * 1.5) {
                this.unloadZone(id);
            }
        });
    }

    async loadZone(zone) {
        // Add to loading queue
        this.loadingQueue.push(zone.id);

        // Load XKT file
        const xktData = await fetch(`/zones/db1112/${zone.xktFile}`);
        const model = await this.parseXKT(xktData);

        // Add to scene
        this.scene.addModel(zone.id, model);
        this.loadedZones.set(zone.id, model);

        // Remove from loading queue
        this.loadingQueue = this.loadingQueue.filter(id => id !== zone.id);
    }

    unloadZone(zoneId) {
        const model = this.loadedZones.get(zoneId);
        if (model) {
            this.scene.removeModel(zoneId);
            this.loadedZones.delete(zoneId);
        }
    }
}
```

## 3. Implementation Steps

### Step 1: Zone Data Extraction
```rust
// Query all zones in database 1112
let zones = query_zones(1112)?;

// For each zone, get elements and bounding box
for zone in zones {
    let elements = query_zone_elements(zone.refno)?;
    let bbox = calculate_bounding_box(&elements)?;
    zone_info.push(ZoneInfo {
        refno: zone.refno,
        bbox,
        element_count: elements.len(),
    });
}
```

### Step 2: Batch XKT Generation
```rust
// Generate XKT for each zone
for zone_info in zones {
    let xkt_data = generate_xkt(
        dbno: 1112,
        refno: zone_info.refno,
        compress: true,
    )?;

    // Save to file
    let filename = format!("output/zones/db1112/zone_{}.xkt", zone_info.id);
    fs::write(filename, xkt_data)?;
}
```

### Step 3: Create Spatial Index
```javascript
// Generate zone manifest with spatial information
const manifest = {
    database: 1112,
    zones: zones.map(zone => ({
        id: zone.id,
        name: zone.name,
        refno: zone.refno,
        boundingBox: zone.bbox,
        center: calculateCenter(zone.bbox),
        radius: calculateRadius(zone.bbox),
        elementCount: zone.elementCount,
        fileSize: zone.fileSize,
        xktFile: `zone_${zone.id}.xkt`,
        compressed: true
    }))
};

fs.writeFileSync('output/zones/db1112/zone_manifest.json', JSON.stringify(manifest));
```

### Step 4: Client Integration
```html
<!-- viewer.html -->
<script>
    const viewer = new XeokitViewer();
    const zoneLoader = new ZoneLoader(viewer.camera, viewer.scene);

    // Load zone manifest
    const manifest = await fetch('/zones/db1112/zone_manifest.json');
    zoneLoader.setManifest(manifest);

    // Initial load
    zoneLoader.updateVisibleZones();

    // Update on camera move
    viewer.camera.on('changed', () => {
        zoneLoader.updateVisibleZones();
    });
</script>
```

## 4. Optimization Strategies

### 4.1 Progressive Loading
- Load low-res preview first, then high-res model
- Use placeholder bounding boxes during loading
- Implement cancellable loading for quick camera movements

### 4.2 Memory Management
- Set maximum loaded zones limit
- Implement LRU cache for zone models
- Clear WebGL buffers when unloading

### 4.3 Network Optimization
- Use HTTP/2 for parallel zone loading
- Implement request batching
- Add server-side caching headers

### 4.4 Performance Monitoring
```javascript
class PerformanceMonitor {
    constructor() {
        this.metrics = {
            loadedZones: 0,
            totalMemory: 0,
            loadTime: [],
            frameRate: 0
        };
    }

    trackZoneLoad(zoneId, loadTime, memoryUsage) {
        this.metrics.loadedZones++;
        this.metrics.totalMemory += memoryUsage;
        this.metrics.loadTime.push({
            zone: zoneId,
            time: loadTime
        });
    }
}
```

## 5. Testing Strategy

### 5.1 Unit Tests
- Test zone boundary calculation
- Test frustum culling algorithm
- Test XKT generation for each zone

### 5.2 Integration Tests
- Test loading multiple zones
- Test memory limits
- Test network failures

### 5.3 Performance Tests
- Measure loading times
- Test with different view distances
- Benchmark memory usage

## 6. Expected Benefits

1. **Reduced Initial Load Time**: Only load visible zones
2. **Better Memory Usage**: Unload distant zones
3. **Improved Frame Rate**: Less geometry to render
4. **Scalability**: Can handle very large models
5. **Network Efficiency**: Load only what's needed

## 7. Potential Challenges

1. **Zone Boundary Handling**: Elements spanning multiple zones
2. **Loading Latency**: Delay when moving camera quickly
3. **Memory Fragmentation**: Frequent loading/unloading
4. **Coordination Complexity**: Managing multiple async loads

## 8. Next Steps

1. Query all zones in database 1112
2. Implement zone XKT generator
3. Create zone manifest generator
4. Build client-side zone loader
5. Test with real data
6. Optimize based on performance metrics