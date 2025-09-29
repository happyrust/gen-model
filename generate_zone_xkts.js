#!/usr/bin/env node

import fetch from 'node-fetch';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

/**
 * Generate XKT files for all zones in database 1112
 */
class ZoneXKTGenerator {
    constructor(baseUrl = 'http://localhost:8080') {
        this.baseUrl = baseUrl;
        this.dbno = 1112;
        this.outputDir = path.join('output', 'zones', `db${this.dbno}`);
        this.zones = [];
        this.generatedFiles = [];
    }

    /**
     * Define zones with known refnos
     * Since we can't query zones directly, we'll use known refnos
     */
    defineZones() {
        // Based on previous tests, we know these refnos exist
        this.zones = [
            {
                id: 'zone_001',
                name: 'Process Area A',
                refno: '17496/266203',
                description: 'Main process equipment area'
            },
            {
                id: 'zone_002',
                name: 'Utility Area',
                refno: '17497/256215',
                description: 'Utility systems and support equipment'
            },
            {
                id: 'zone_003',
                name: 'Storage Area',
                refno: '17498/266300',  // Example - may need verification
                description: 'Storage tanks and vessels'
            },
            {
                id: 'zone_004',
                name: 'Pipe Rack Zone',
                refno: '17499/256400',  // Example - may need verification
                description: 'Main pipe rack structures'
            },
            {
                id: 'zone_005',
                name: 'Equipment Zone B',
                refno: '17500/266500',  // Example - may need verification
                description: 'Secondary equipment area'
            }
        ];

        console.log(`📋 Defined ${this.zones.length} zones for generation`);
    }

    /**
     * Generate XKT for a single zone
     */
    async generateZoneXKT(zone, compress = true) {
        console.log(`\n🔧 Generating XKT for ${zone.name} (${zone.refno})...`);

        try {
            // Call the XKT generation API
            const response = await fetch(`${this.baseUrl}/api/xkt/generate`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    dbno: this.dbno,
                    refno: zone.refno,
                    compress: compress
                })
            });

            if (!response.ok) {
                const text = await response.text();
                throw new Error(`API error ${response.status}: ${text}`);
            }

            const result = await response.json();

            if (result.success) {
                console.log(`✅ Generated: ${result.filename}`);

                // Download the file
                const downloadUrl = `${this.baseUrl}${result.url}`;
                const xktResponse = await fetch(downloadUrl);

                if (!xktResponse.ok) {
                    throw new Error(`Failed to download XKT file: ${xktResponse.statusText}`);
                }

                const xktData = await xktResponse.buffer();

                // Save to zone output directory
                const zoneFilename = `${zone.id}.xkt`;
                const zonePath = path.join(this.outputDir, zoneFilename);
                fs.writeFileSync(zonePath, xktData);

                const fileSize = xktData.length;
                console.log(`  📁 Saved to: ${zonePath}`);
                console.log(`  📊 File size: ${fileSize.toLocaleString()} bytes`);

                return {
                    zone: zone,
                    filename: zoneFilename,
                    path: zonePath,
                    size: fileSize,
                    compressed: compress,
                    generatedAt: new Date().toISOString()
                };
            } else {
                throw new Error('Generation failed: ' + JSON.stringify(result));
            }
        } catch (error) {
            console.error(`❌ Failed to generate XKT for ${zone.name}: ${error.message}`);
            return null;
        }
    }

    /**
     * Validate generated XKT file
     */
    async validateXKT(filePath) {
        try {
            const buffer = fs.readFileSync(filePath);

            if (buffer.length < 120) {
                throw new Error('File too small to be valid XKT');
            }

            // Read version and compression flag
            const versionAndFlags = buffer.readUInt32LE(0);
            const version = versionAndFlags & 0x7FFFFFFF;
            const compressed = (versionAndFlags & 0x80000000) !== 0;

            // Read section offsets
            const sectionOffsets = [];
            for (let i = 0; i < 29; i++) {
                const offset = buffer.readUInt32LE(4 + i * 4);
                sectionOffsets.push(offset);
            }

            // Check if file has actual data
            const hasData = sectionOffsets.some(offset => offset > 120);

            return {
                valid: hasData,
                version: version,
                compressed: compressed,
                size: buffer.length,
                hasGeometry: hasData
            };
        } catch (error) {
            return {
                valid: false,
                error: error.message
            };
        }
    }

    /**
     * Generate zone manifest
     */
    async generateManifest() {
        console.log('\n📝 Generating zone manifest...');

        const manifest = {
            database: this.dbno,
            totalZones: this.generatedFiles.length,
            generatedAt: new Date().toISOString(),
            zones: []
        };

        for (const file of this.generatedFiles) {
            if (!file) continue;

            // Validate XKT file
            const validation = await this.validateXKT(file.path);

            // Try to get bounding box (would need actual parsing)
            const zoneInfo = {
                id: file.zone.id,
                name: file.zone.name,
                refno: file.zone.refno,
                description: file.zone.description,
                xktFile: file.filename,
                fileSize: file.size,
                compressed: file.compressed,
                hasGeometry: validation.hasGeometry,
                valid: validation.valid,
                generatedAt: file.generatedAt
            };

            // Add placeholder bounding box (would need actual calculation)
            zoneInfo.boundingBox = {
                min: [-100, -100, 0],
                max: [100, 100, 200]
            };
            zoneInfo.center = [0, 0, 100];
            zoneInfo.radius = 173.2;

            manifest.zones.push(zoneInfo);
        }

        // Save manifest
        const manifestPath = path.join(this.outputDir, 'zone_manifest.json');
        fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

        console.log(`✅ Manifest saved to: ${manifestPath}`);
        return manifest;
    }

    /**
     * Generate HTML viewer for zone-based loading
     */
    generateViewer() {
        console.log('\n🌐 Generating zone viewer...');

        const viewerHtml = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>Zone-Based XKT Viewer</title>
    <style>
        body { margin: 0; font-family: Arial; }
        #viewer { width: 100%; height: calc(100vh - 150px); }
        #controls {
            position: absolute;
            top: 10px;
            left: 10px;
            background: rgba(255, 255, 255, 0.9);
            padding: 15px;
            border-radius: 5px;
            box-shadow: 0 2px 5px rgba(0,0,0,0.2);
        }
        #info {
            position: absolute;
            bottom: 10px;
            left: 10px;
            background: rgba(0, 0, 0, 0.8);
            color: white;
            padding: 10px;
            border-radius: 5px;
            font-size: 12px;
        }
        .zone-item {
            margin: 5px 0;
            padding: 5px;
            background: #f0f0f0;
            border-radius: 3px;
        }
        .zone-item.loaded {
            background: #d4edda;
        }
        .zone-item.loading {
            background: #fff3cd;
        }
    </style>
</head>
<body>
    <div id="controls">
        <h3>Zone-Based Loading Control</h3>
        <div>
            <label>View Distance: <input type="range" id="viewDistance" min="100" max="1000" value="500">
                <span id="viewDistanceValue">500</span>m
            </label>
        </div>
        <div>
            <label>Auto Load: <input type="checkbox" id="autoLoad" checked></label>
        </div>
        <h4>Zones:</h4>
        <div id="zoneList"></div>
    </div>

    <div id="info">
        <div>Loaded Zones: <span id="loadedCount">0</span></div>
        <div>Total Memory: <span id="memoryUsage">0</span> MB</div>
        <div>FPS: <span id="fps">0</span></div>
    </div>

    <canvas id="viewer"></canvas>

    <script type="module">
        // Zone loading manager
        class ZoneManager {
            constructor() {
                this.manifest = null;
                this.loadedZones = new Map();
                this.loadingQueue = [];
                this.viewDistance = 500;
                this.autoLoad = true;
            }

            async init() {
                // Load manifest
                const response = await fetch('/zones/db1112/zone_manifest.json');
                this.manifest = await response.json();

                console.log('Zone manifest loaded:', this.manifest);
                this.displayZones();
            }

            displayZones() {
                const listEl = document.getElementById('zoneList');
                listEl.innerHTML = '';

                this.manifest.zones.forEach(zone => {
                    const div = document.createElement('div');
                    div.className = 'zone-item';
                    div.id = 'zone-' + zone.id;
                    div.innerHTML = \`
                        <strong>\${zone.name}</strong> (\${zone.id})<br>
                        Size: \${(zone.fileSize / 1024).toFixed(1)} KB
                        <button onclick="zoneManager.toggleZone('\${zone.id}')">Load/Unload</button>
                    \`;
                    listEl.appendChild(div);
                });
            }

            async loadZone(zoneId) {
                const zone = this.manifest.zones.find(z => z.id === zoneId);
                if (!zone || this.loadedZones.has(zoneId)) return;

                console.log('Loading zone:', zone.name);

                // Update UI
                const zoneEl = document.getElementById('zone-' + zoneId);
                if (zoneEl) zoneEl.classList.add('loading');

                try {
                    // Simulate XKT loading
                    const response = await fetch(\`/zones/db1112/\${zone.xktFile}\`);
                    const data = await response.arrayBuffer();

                    // Here would parse and add to 3D scene
                    this.loadedZones.set(zoneId, {
                        zone: zone,
                        data: data,
                        size: data.byteLength
                    });

                    // Update UI
                    if (zoneEl) {
                        zoneEl.classList.remove('loading');
                        zoneEl.classList.add('loaded');
                    }

                    this.updateStats();
                    console.log('Zone loaded:', zone.name);
                } catch (error) {
                    console.error('Failed to load zone:', error);
                    if (zoneEl) zoneEl.classList.remove('loading');
                }
            }

            unloadZone(zoneId) {
                if (!this.loadedZones.has(zoneId)) return;

                console.log('Unloading zone:', zoneId);
                this.loadedZones.delete(zoneId);

                // Update UI
                const zoneEl = document.getElementById('zone-' + zoneId);
                if (zoneEl) {
                    zoneEl.classList.remove('loaded');
                }

                this.updateStats();
            }

            toggleZone(zoneId) {
                if (this.loadedZones.has(zoneId)) {
                    this.unloadZone(zoneId);
                } else {
                    this.loadZone(zoneId);
                }
            }

            updateStats() {
                document.getElementById('loadedCount').textContent = this.loadedZones.size;

                let totalMemory = 0;
                this.loadedZones.forEach(zone => {
                    totalMemory += zone.size;
                });
                document.getElementById('memoryUsage').textContent =
                    (totalMemory / (1024 * 1024)).toFixed(2);
            }
        }

        // Initialize
        const zoneManager = new ZoneManager();
        window.zoneManager = zoneManager;

        zoneManager.init().then(() => {
            console.log('Zone manager initialized');

            // Load first zone by default
            if (zoneManager.manifest.zones.length > 0) {
                zoneManager.loadZone(zoneManager.manifest.zones[0].id);
            }
        });

        // Controls
        document.getElementById('viewDistance').addEventListener('input', (e) => {
            zoneManager.viewDistance = parseInt(e.target.value);
            document.getElementById('viewDistanceValue').textContent = e.target.value;
        });

        document.getElementById('autoLoad').addEventListener('change', (e) => {
            zoneManager.autoLoad = e.target.checked;
        });

        // FPS counter
        let lastTime = performance.now();
        let frameCount = 0;

        function updateFPS() {
            frameCount++;
            const currentTime = performance.now();
            if (currentTime - lastTime > 1000) {
                document.getElementById('fps').textContent = frameCount;
                frameCount = 0;
                lastTime = currentTime;
            }
            requestAnimationFrame(updateFPS);
        }
        updateFPS();
    </script>
</body>
</html>`;

        const viewerPath = path.join(this.outputDir, 'zone_viewer.html');
        fs.writeFileSync(viewerPath, viewerHtml);

        console.log(`✅ Viewer saved to: ${viewerPath}`);
    }

    /**
     * Main execution
     */
    async run() {
        console.log('='.repeat(60));
        console.log('🏗️  Zone-Based XKT Generation');
        console.log('='.repeat(60));

        // Create output directory
        if (!fs.existsSync(this.outputDir)) {
            fs.mkdirSync(this.outputDir, { recursive: true });
            console.log(`📁 Created output directory: ${this.outputDir}`);
        }

        // Define zones
        this.defineZones();

        // Generate XKT for each zone
        console.log(`\n🚀 Starting XKT generation for ${this.zones.length} zones...`);

        for (const zone of this.zones) {
            // Try compressed first
            const result = await this.generateZoneXKT(zone, true);

            if (result) {
                this.generatedFiles.push(result);

                // Validate the generated file
                const validation = await this.validateXKT(result.path);
                if (validation.valid && validation.hasGeometry) {
                    console.log(`  ✅ Validation passed: Has geometry`);
                } else if (validation.valid && !validation.hasGeometry) {
                    console.log(`  ⚠️ Valid XKT but no geometry data`);
                } else {
                    console.log(`  ❌ Validation failed: ${validation.error}`);
                }
            }

            // Small delay to avoid overwhelming the server
            await new Promise(resolve => setTimeout(resolve, 1000));
        }

        // Generate manifest
        const manifest = await this.generateManifest();

        // Generate viewer
        this.generateViewer();

        // Summary
        console.log('\n' + '='.repeat(60));
        console.log('📊 Generation Summary:');
        console.log('='.repeat(60));
        console.log(`Total zones attempted: ${this.zones.length}`);
        console.log(`Successfully generated: ${this.generatedFiles.filter(f => f !== null).length}`);
        console.log(`Failed: ${this.zones.length - this.generatedFiles.filter(f => f !== null).length}`);

        let totalSize = 0;
        this.generatedFiles.forEach(file => {
            if (file) totalSize += file.size;
        });
        console.log(`Total size: ${(totalSize / 1024).toFixed(2)} KB`);

        console.log('\n✅ Zone XKT generation complete!');
        console.log(`📁 Files saved to: ${this.outputDir}`);
        console.log(`🌐 Open zone_viewer.html to test zone-based loading`);

        return manifest;
    }
}

// Run if executed directly
if (import.meta.url === `file://${process.argv[1]}`) {
    const generator = new ZoneXKTGenerator();
    generator.run().catch(console.error);
}

export { ZoneXKTGenerator };