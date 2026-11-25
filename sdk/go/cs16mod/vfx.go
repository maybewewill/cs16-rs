//go:build wasm

package cs16mod

// VFX provides 1-line strongly-typed Protocol 48 TE_* temp-entity VFX emitters.
var VFX = vfxAPI{}

type vfxAPI struct{}

// ShockwaveRing emits an expanding TE_BEAMCYLINDER ring (Frost Nade, level-up, heal wave).
func (vfxAPI) ShockwaveRing(origin Vec3, radius float32, col Color, lifeDecisec, width, slot int32) int32 {
	return rawVFXShockwaveRing(origin.X, origin.Y, origin.Z, radius, col.PackRGBA(), lifeDecisec, width, slot)
}

// LaserBeam emits a TE_BEAMPOINTS laser/tripwire beam between start and end.
func (vfxAPI) LaserBeam(start, end Vec3, col Color, lifeDecisec, width, noise, slot int32) int32 {
	return rawVFXLaserBeam(start.X, start.Y, start.Z, end.X, end.Y, end.Z, col.PackRGBA(), lifeDecisec, width, noise, slot)
}

// FollowTrail attaches a TE_BEAMFOLLOW ribbon trail to entIdx.
func (vfxAPI) FollowTrail(entIdx int32, col Color, lifeDecisec, width, slot int32) int32 {
	return rawVFXFollowTrail(entIdx, col.PackRGBA(), lifeDecisec, width, slot)
}

// KillBeam removes all beams attached to entIdx.
func (vfxAPI) KillBeam(entIdx, slot int32) int32 {
	return rawVFXKillBeam(entIdx, slot)
}

// AuraLight emits a TE_DLIGHT dynamic point light around origin.
func (vfxAPI) AuraLight(origin Vec3, radius int32, col Color, lifeDecisec, decay, slot int32) int32 {
	return rawVFXAuraLight(origin.X, origin.Y, origin.Z, radius, col.PackRGB(), lifeDecisec, decay, slot)
}

// Sparks emits a TE_SPARKS metallic spark shower.
func (vfxAPI) Sparks(origin Vec3, slot int32) int32 {
	return rawVFXSparks(origin.X, origin.Y, origin.Z, slot)
}

// Implosion emits a TE_IMPLOSION inward particle burst.
func (vfxAPI) Implosion(origin Vec3, radius, count, lifeDecisec, slot int32) int32 {
	return rawVFXImplosion(origin.X, origin.Y, origin.Z, radius, count, lifeDecisec, slot)
}

// TeleportSplash emits a TE_TELEPORT particle burst.
func (vfxAPI) TeleportSplash(origin Vec3, slot int32) int32 {
	return rawVFXTeleportSplash(origin.X, origin.Y, origin.Z, slot)
}

// LavaSplash emits a TE_LAVASPLASH red/orange fountain.
func (vfxAPI) LavaSplash(origin Vec3, slot int32) int32 {
	return rawVFXLavaSplash(origin.X, origin.Y, origin.Z, slot)
}

// Explosion emits a TE_EXPLOSION fireball and shockwave.
func (vfxAPI) Explosion(origin Vec3, scale, flags, slot int32) int32 {
	return rawVFXExplosion(origin.X, origin.Y, origin.Z, scale, flags, slot)
}

// BloodStream emits a directional TE_BLOODSTREAM jet.
func (vfxAPI) BloodStream(origin, dir Vec3, colorIdx, speed, slot int32) int32 {
	return rawVFXBloodStream(origin.X, origin.Y, origin.Z, dir.X, dir.Y, dir.Z, colorIdx, speed, slot)
}

// WorldDecal stamps a permanent WAD decal (e.g. "{scorch1") onto world geometry.
func (vfxAPI) WorldDecal(origin Vec3, decalName string, slot int32) int32 {
	p, n := strArg(decalName)
	return rawVFXWorldDecal(origin.X, origin.Y, origin.Z, p, n, slot)
}
