import java.io.File
import org.json.JSONObject
import org.ekiben.kaisatsu.KaisatsuException
import org.ekiben.kaisatsu.Verifier
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

private fun hex(text: String) = ByteArray(text.length / 2) { text.substring(2 * it, 2 * it + 2).toInt(16).toByte() }

class VectorsTest {
    private val vectors = JSONObject(File(System.getProperty("vectors")).readText())

    @Test
    fun everyVectorGivesTheExpectedResult() {
        val keys = vectors.getJSONArray("keys")
        val trusted = (0 until keys.length()).map { keys.getJSONObject(it) }.first { it.getBoolean("trusted") }
        val verifier = Verifier(listOf(hex(trusted.getString("public_key"))))

        val all = vectors.getJSONArray("vectors")
        for (index in 0 until all.length()) {
            val vector = all.getJSONObject(index)
            val name = vector.getString("name")
            val ticket = hex(vector.getString("ticket"))
            val claims = vector.optJSONObject("claims")
            val checks = vector.optJSONArray("time_checks")?.let { array ->
                (0 until array.length()).map { array.getJSONObject(it).getLong("now") to array.getJSONObject(it).getString("expect") }
            } ?: listOf((claims?.getLong("valid_from") ?: 0L) to vector.getString("expect"))

            for ((now, expect) in checks) {
                if (expect == "Ok") {
                    val result = verifier.verify(ticket, now.toULong())
                    assertEquals(claims!!.getString("ticket_id"), result.ticketId, name)
                    assertEquals(claims.getString("issuer"), result.issuer, name)
                    assertEquals(claims.getLong("valid_until").toULong(), result.validUntil, name)
                } else {
                    val error = assertFailsWith<KaisatsuException>("$name at $now") { verifier.verify(ticket, now.toULong()) }
                    assertEquals(expect, error::class.simpleName, "$name at $now")
                }
            }
        }
    }

    @Test
    fun unusableKeysAreRejectedUpFront() {
        assertFailsWith<KaisatsuException.InvalidKey> { Verifier(listOf(ByteArray(32))) }
        assertFailsWith<KaisatsuException.InvalidKey> { Verifier(listOf(ByteArray(31))) }
    }
}
