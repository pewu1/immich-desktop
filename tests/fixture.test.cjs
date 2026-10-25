const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const crypto = require('node:crypto');
test('receiver expectations and embedded drag fixture have identical content', () => {
  const expected = JSON.parse(fs.readFileSync('fixtures/fixture.json'));
  const fixture = fs.readFileSync('fixtures/drag-test.png');
  assert.equal(crypto.createHash('sha256').update(fixture).digest('hex'), expected.sha256);
  assert.equal(fixture.length, expected.size);
  assert.ok(fixture.equals(fs.readFileSync('ui/drag-test.png')));
  assert.deepEqual(expected, JSON.parse(fs.readFileSync('ui/fixture.json')));
});
