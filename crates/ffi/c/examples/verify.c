/*
 * A minimal ticket gate: verifies one ticket against one trusted key.
 *
 *     verify <public-key-hex> <ticket-hex>
 *
 * The ticket is its raw bytes, here written as hex; how a real gate receives them (a binary
 * QR code, Base64, …) is up to the integration.
 *
 * Exits 0 and prints the ticket id when the ticket is authentic and currently valid,
 * otherwise prints the rejection (e.g. "ピンポーン🔔 BadSignature") and exits 1.
 * Everything beyond this — re-entry rules, zones, logging — is up to your gate.
 */
#include <stdio.h>
#include <string.h>

#include "kaisatsu.h"

/* Decodes `hex` into `out`; returns the number of bytes, or -1 if it is not hex or too long. */
static long parse_hex(const char *hex, uint8_t *out, size_t capacity) {
    size_t len = strlen(hex);
    if (len % 2 != 0 || len / 2 > capacity) return -1;
    for (size_t i = 0; i < len / 2; i++) {
        unsigned int byte;
        if (sscanf(hex + 2 * i, "%2x", &byte) != 1) return -1;
        out[i] = (uint8_t)byte;
    }
    return (long)(len / 2);
}

static void print_uuid(const uint8_t id[16]) {
    for (size_t i = 0; i < 16; i++) {
        if (i == 4 || i == 6 || i == 8 || i == 10) putchar('-');
        printf("%02x", id[i]);
    }
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s <public-key-hex> <ticket-hex>\n", argv[0]);
        return 2;
    }

    KaisatsuKey key;
    if (parse_hex(argv[1], key.public_key, sizeof key.public_key) != 32 ||
        kaisatsu_key_check(&key, NULL) != KAISATSU_STATUS_OK) {
        fprintf(stderr, "invalid public key\n");
        return 2;
    }

    uint8_t bytes[KAISATSU_MAX_TICKET_LEN];
    long len = parse_hex(argv[2], bytes, sizeof bytes);
    if (len < 0) {
        fprintf(stderr, "the ticket is not hex, or longer than KAISATSU_MAX_TICKET_LEN\n");
        return 2;
    }

    KaisatsuTicket ticket;
    KaisatsuStatus status = kaisatsu_verify(&key, 1, bytes, (size_t)len, &ticket);
    if (status != KAISATSU_STATUS_OK) {
        printf("%s\n", kaisatsu_status_message(status));
        return 1;
    }

    printf("OK ticket ");
    print_uuid(ticket.ticket_id);
    printf(" issued by %.*s", (int)ticket.issuer_len, (const char *)ticket.issuer);

    const uint8_t *zone;
    size_t zone_len;
    if (kaisatsu_ticket_extension(&ticket, 0x80, &zone, &zone_len)) {
        printf(" zone %.*s", (int)zone_len, (const char *)zone);
    }
    printf("\n");
    return 0;
}
