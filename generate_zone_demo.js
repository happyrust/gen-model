#!/usr/bin/env node

import fetch from 'node-fetch';
import fs from 'fs';
import path from 'path';

/**
 * Simplified demo for zone-based XKT generation
 * Using only known valid refno for demonstration
 */

async function generateDemoZones() {
    const baseUrl = 'http://localhost:8080';
    const dbno = 1112;
    const outputDir = path.join('output', 'zones', `db${dbno}`);

    // Create output directory
    if (!fs.existsSync(outputDir)) {
        fs.mkdirSync(outputDir, { recursive: true });
    }

    // Use the one known valid zone
    const validZone = {
        id: 'zone_001',
        name: 'Process Area A',
        refno: '17496/266203',
        description: 'Main process equipment area'
    };

    console.log('🏗️ Zone-Based XKT Generation Demo');
    console.log('='.repeat(50));
    console.log(`Generating XKT for: ${validZone.name}`);

    try {
        // Generate XKT
        const response = await fetch(`${baseUrl}/api/xkt/generate`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                dbno: dbno,
                refno: validZone.refno,
                compress: true
            })
        });

        if (!response.ok) {
            throw new Error(`Generation failed: ${response.statusText}`);
        }

        const result = await response.json();
        console.log(`✅ Generated: ${result.filename}`);

        // Download file
        const downloadResponse = await fetch(`${baseUrl}${result.url}`);
        const xktData = await downloadResponse.arrayBuffer();
        const buffer = Buffer.from(xktData);

        // Save zone file
        const zonePath = path.join(outputDir, `${validZone.id}.xkt`);
        fs.writeFileSync(zonePath, buffer);
        console.log(`📁 Saved to: ${zonePath}`);
        console.log(`📊 Size: ${buffer.length} bytes`);

        // Create a simple manifest
        const manifest = {
            database: dbno,
            totalZones: 1,
            generatedAt: new Date().toISOString(),
            zones: [{
                id: validZone.id,
                name: validZone.name,
                refno: validZone.refno,
                xktFile: `${validZone.id}.xkt`,
                fileSize: buffer.length,
                compressed: true,
                // Placeholder bounding box
                boundingBox: {
                    min: [-10, -10, -10],
                    max: [10, 10, 10]
                },
                center: [0, 0, 0],
                radius: 17.32
            }]
        };

        // Save manifest
        const manifestPath = path.join(outputDir, 'zone_manifest.json');
        fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
        console.log(`📝 Manifest saved to: ${manifestPath}`);

        console.log('\n✅ Demo zone generation complete!');
        console.log('\n📋 Implementation Plan for Full System:');
        console.log('1. Query database for all ZONE elements');
        console.log('2. Get refno for each zone');
        console.log('3. Generate XKT for zones with geometry');
        console.log('4. Create spatial index with bounding boxes');
        console.log('5. Implement view-based loading in client');

        return manifest;

    } catch (error) {
        console.error('❌ Error:', error.message);
        return null;
    }
}

// Run
generateDemoZones().catch(console.error);