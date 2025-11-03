#!/usr/bin/env node

import fetch from 'node-fetch';
import fs from 'fs';
import path from 'path';

/**
 * Query all zones in database 1112 and prepare for XKT generation
 */
class ZoneQueryManager {
    constructor(baseUrl = 'http://localhost:8080') {
        this.baseUrl = baseUrl;
        this.dbno = 1112;
        this.zones = [];
    }

    async queryAllZones() {
        console.log('🔍 Querying all zones in database', this.dbno);

        try {
            // First, get the database hierarchy to find all zones
            const response = await fetch(`${this.baseUrl}/api/query`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    dbno: this.dbno,
                    refno: '/',  // Root query
                    query_type: 'hierarchy',
                    depth: 2  // Get SITE -> ZONE level
                })
            });

            if (!response.ok) {
                throw new Error(`Failed to query database: ${response.statusText}`);
            }

            const data = await response.json();

            // Extract zones from hierarchy
            this.zones = this.extractZones(data);

            console.log(`✅ Found ${this.zones.length} zones in database ${this.dbno}`);
            return this.zones;
        } catch (error) {
            console.error('❌ Error querying zones:', error);
            throw error;
        }
    }

    extractZones(hierarchy) {
        const zones = [];

        // Recursively find all ZONE elements
        const findZones = (node, parentPath = '') => {
            if (!node) return;

            // Check if this is a ZONE
            if (node.type === 'ZONE' || node.name?.startsWith('ZONE')) {
                zones.push({
                    name: node.name || `ZONE_${zones.length + 1}`,
                    refno: node.refno || node.ref,
                    path: parentPath + '/' + node.name,
                    elementCount: node.children?.length || 0,
                    attributes: node.attributes || {}
                });
            }

            // Recursively check children
            if (node.children && Array.isArray(node.children)) {
                node.children.forEach(child => {
                    findZones(child, parentPath + '/' + (node.name || ''));
                });
            }
        };

        findZones(hierarchy);
        return zones;
    }

    async getZoneDetails(zone) {
        console.log(`📊 Getting details for zone: ${zone.name}`);

        try {
            // Query zone elements and bounding box
            const response = await fetch(`${this.baseUrl}/api/query`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    dbno: this.dbno,
                    refno: zone.refno,
                    query_type: 'elements',
                    include_bbox: true
                })
            });

            if (!response.ok) {
                console.warn(`⚠️ Could not get details for zone ${zone.name}`);
                return null;
            }

            const data = await response.json();

            return {
                ...zone,
                elementCount: data.element_count || data.elements?.length || 0,
                boundingBox: data.bounding_box || this.calculateBBox(data.elements),
                elements: data.elements || []
            };
        } catch (error) {
            console.error(`❌ Error getting zone details for ${zone.name}:`, error);
            return null;
        }
    }

    calculateBBox(elements) {
        if (!elements || elements.length === 0) {
            return {
                min: [0, 0, 0],
                max: [0, 0, 0]
            };
        }

        let minX = Infinity, minY = Infinity, minZ = Infinity;
        let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;

        elements.forEach(elem => {
            if (elem.position) {
                minX = Math.min(minX, elem.position[0]);
                minY = Math.min(minY, elem.position[1]);
                minZ = Math.min(minZ, elem.position[2]);
                maxX = Math.max(maxX, elem.position[0]);
                maxY = Math.max(maxY, elem.position[1]);
                maxZ = Math.max(maxZ, elem.position[2]);
            }
        });

        return {
            min: [minX, minY, minZ],
            max: [maxX, maxY, maxZ]
        };
    }

    async generateZoneManifest() {
        console.log('📝 Generating zone manifest...');

        const zoneDetails = [];

        // Get details for each zone
        for (const zone of this.zones) {
            const details = await this.getZoneDetails(zone);
            if (details) {
                zoneDetails.push({
                    id: `zone_${String(zoneDetails.length + 1).padStart(3, '0')}`,
                    name: details.name,
                    refno: details.refno,
                    boundingBox: details.boundingBox,
                    center: this.calculateCenter(details.boundingBox),
                    radius: this.calculateRadius(details.boundingBox),
                    elementCount: details.elementCount,
                    xktFile: `zone_${String(zoneDetails.length + 1).padStart(3, '0')}.xkt`
                });
            }
        }

        const manifest = {
            database: this.dbno,
            totalZones: zoneDetails.length,
            generatedAt: new Date().toISOString(),
            zones: zoneDetails
        };

        // Create output directory
        const outputDir = path.join('output', 'zones', `db${this.dbno}`);
        if (!fs.existsSync(outputDir)) {
            fs.mkdirSync(outputDir, { recursive: true });
        }

        // Write manifest
        const manifestPath = path.join(outputDir, 'zone_manifest.json');
        fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

        console.log(`✅ Zone manifest saved to ${manifestPath}`);
        console.log(`📊 Total zones: ${manifest.totalZones}`);

        return manifest;
    }

    calculateCenter(bbox) {
        if (!bbox || !bbox.min || !bbox.max) {
            return [0, 0, 0];
        }
        return [
            (bbox.min[0] + bbox.max[0]) / 2,
            (bbox.min[1] + bbox.max[1]) / 2,
            (bbox.min[2] + bbox.max[2]) / 2
        ];
    }

    calculateRadius(bbox) {
        if (!bbox || !bbox.min || !bbox.max) {
            return 0;
        }
        const dx = bbox.max[0] - bbox.min[0];
        const dy = bbox.max[1] - bbox.min[1];
        const dz = bbox.max[2] - bbox.min[2];
        return Math.sqrt(dx*dx + dy*dy + dz*dz) / 2;
    }

    async testQueryWithKnownRefno() {
        console.log('🧪 Testing with known refno 17496/266203...');

        try {
            const response = await fetch(`${this.baseUrl}/api/query`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    dbno: this.dbno,
                    refno: '17496/266203',
                    query_type: 'hierarchy'
                })
            });

            if (!response.ok) {
                console.error('❌ Test query failed:', response.statusText);
                return false;
            }

            const data = await response.json();
            console.log('✅ Test query successful');
            console.log('📊 Response structure:', Object.keys(data));

            // Try to understand the structure
            if (data.name) console.log('  Name:', data.name);
            if (data.type) console.log('  Type:', data.type);
            if (data.refno) console.log('  Refno:', data.refno);
            if (data.children) console.log('  Children count:', data.children.length);

            return true;
        } catch (error) {
            console.error('❌ Test query error:', error);
            return false;
        }
    }
}

// Main execution
async function main() {
    const manager = new ZoneQueryManager();

    console.log('='.repeat(60));
    console.log('🏗️  Zone-Based XKT Generation Preparation');
    console.log('='.repeat(60));

    // First test with known refno
    await manager.testQueryWithKnownRefno();

    console.log('\n' + '-'.repeat(60));

    // Query all zones
    await manager.queryAllZones();

    // Generate manifest if zones found
    if (manager.zones.length > 0) {
        const manifest = await manager.generateZoneManifest();
        console.log('\n📋 Zone Manifest Summary:');
        manifest.zones.forEach(zone => {
            console.log(`  ${zone.id}: ${zone.name} (${zone.elementCount} elements)`);
        });
    } else {
        console.log('⚠️ No zones found. Using fallback approach...');

        // Fallback: Create zones based on known refnos
        const knownZones = [
            { name: 'ZONE_001', refno: '17496/266203' },
            { name: 'ZONE_002', refno: '17497/256215' }
        ];

        manager.zones = knownZones;
        const manifest = await manager.generateZoneManifest();
        console.log('✅ Created fallback manifest with known zones');
    }

    console.log('\n' + '='.repeat(60));
    console.log('✅ Zone query complete!');
}

// Run if executed directly
if (import.meta.url === `file://${process.argv[1]}`) {
    main().catch(console.error);
}

export { ZoneQueryManager };